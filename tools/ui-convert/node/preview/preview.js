// UI v2 skin preview script. One classic script, no dependencies, no build step.
//
// The converter's local server writes <script src="/__ui2/preview.js"> into every HTML page of a skin.
// The script turns the static mockup into the page the converter reads:
//
//   1. data-include is expanded (the element is replaced by the markup of the part, fetched relative to
//      the skin root; parts may include parts; a cycle is an error entry).
//   2. Every element below <body> is numbered: data-ui2-node="<tpl>:<idx>" (see "Numbering").
//   3. The sample (sample/<state>.json) and the texts (l10n/<lang>.toml) are loaded and the bindings are
//      evaluated: data-bind-value, -if, -for (+ -key, -sort, -sort-desc, -filter), -class-toggle,
//      -style-<prop>, -src, -disabled, data-l10n, data-l10n-args and data-hotkey-label (the name of the key
//      of the nearest data-hotkey, `key.<code>` of the texts or the runtime's default). Elements created by a `for` carry
//      data-ui2-inst="<key>/<key>" (the keys of the enclosing loops, outermost first). `state.*` reads the
//      skin's defaults (`skin.toml [state]`, written in front of this script by the server as
//      window.__ui2skin = { toml }) with the sample's `state` written over them, as the runtime mounts them.
//   4. Problems never throw: they are appended to window.__ui2preview.errors as
//      { code, level, src, node, attr, message } where src is the element's data-ui2-src.
//
// Events and actions (data-on-*), script islands and the skin's own JS are not run here.
//
// Modes (a global set before this script loads, or a URL parameter; the global wins):
//   window.__UI2_PREVIEW = { mode, state, lang, root }      ?ui2mode=  ?ui2state=  ?ui2lang=
//   layout     (default) the honest sample: `if` follows its condition, `for` has one instance per item.
//   extract    coverage for the converter: every `if` is drawn, every `for` has exactly one instance.
//   templates  includes are expanded and nodes are numbered, nothing else: no data, no bindings.
//   state: the sample file name, default "hud". lang: the table file name, default "en_US".
//   root: the URL of the skin root (default: the origin root for http, the page folder for file:).
//
// Numbering. A template is the document (everything below <body>, <body> itself is not a node), the
// subtree of an element with data-bind-for or data-bind-if, or the one root element of a
// <template data-layer="tooltip"> (numbered where the <template> stands; extract mode draws it there, marked
// data-ui2-tooltip, and the other modes leave it in the <template>). An element's number is its preorder index
// among the elements of its own template; the root of a nested template belongs to that template (as
// element 0), not to its parent. Templates are numbered in the order their roots appear in the
// document, the document being 0. <svg> is a node but its content is not; <script>, <style>,
// <template>, <link>, <meta> and everything under data-ui2-dev are not nodes. Numbers are written
// before any binding runs, so they do not depend on the data.
//
// Public surface: window.__ui2preview = { version, mode, state, lang, errors, ready, rerender(arg), core }.
//   ready      a promise that resolves (never rejects) when the first render is done.
//   rerender   rerender()            renders again with the data already loaded;
//              rerender("<state>")   loads sample/<state>.json first;
//              rerender({ model, state, ui, anchors })  uses that sample object.
//              Resolves to the errors array.
//   core       the pure part below: expression language, templates, attribute grammars, TOML subset,
//              text engine subset. It has no DOM access; tests run it in plain Node.
//
// l10n: the game's text engine (`loc`) has many transforms. The preview implements {N} (with the
// `_Name` comment the engine allows after the number), the default number format (grouping, up to three
// decimals) and the `plural` transform for English. Everything else ({N: number ...}, gender, upper, ...)
// is left as written and reported as PV-006: not previewed.
(function () {
  "use strict";

  const VERSION = 1;
  const MAX_DEPTH = 128;
  const SAFE = Number.MAX_SAFE_INTEGER;

  // ------------------------------------------------------------------------------------------------
  // Values. Integers are JS numbers (exact below 2^53, which the language contract allows), floats are
  // Flt, text and enum values are strings, a color is Col, a list is Lst (its items are kept for `for`),
  // a record that is present is Struct.
  // ------------------------------------------------------------------------------------------------

  class Flt { constructor(v) { this.v = v; } }
  class Col { constructor(c) { this.c = c >>> 0; } }
  class Lst { constructor(n, items) { this.n = n; this.items = items || null; } }
  class Struct { constructor(obj) { this.obj = obj === undefined ? null : obj; } }

  class ExprError extends Error {
    constructor(code, message, at) {
      super(message);
      this.code = code;
      this.at = at === undefined ? null : at; // index in the source string (UTF-16 units)
    }
  }

  const E = {
    SYNTAX: "E4-001", UNKNOWN_FUNC: "E4-002", ARGC: "E4-003", ONE_EXPR: "E4-020", FOR: "E4-021",
    CLASS_TOGGLE: "E4-024", TEXT_SOURCE: "E4-036", IF_FOR: "E4-037", INCLUDE: "E1-027", UNCLOSED: "E4-013",
    NOT_LIST: "E4-009", DUP_ATTR: "E1-026", HOTKEY: "E7-013", TOOLTIP: "E7-006",
  };
  const PV = {
    INCLUDE_FAILED: "PV-001", INCLUDE_CYCLE: "PV-002", LOAD_FAILED: "PV-003", MODE: "PV-004",
    L10N_MISSING: "PV-005", L10N_UNSUPPORTED: "PV-006", KEY_DUPLICATE: "PV-007", WARN: "PV-008",
    TOML: "PV-009",
  };

  // ------------------------------------------------------------------------------------------------
  // Lexer and parser (port of the Rust implementation; the shared vectors pin the behaviour).
  // ------------------------------------------------------------------------------------------------

  const TWO = ["||", "&&", "==", "!=", "<=", ">="];
  const ONE = "<>+-*/%!?:(),.";
  const isDigit = (c) => c >= "0" && c <= "9";
  const isIdentStart = (c) => (c >= "a" && c <= "z") || (c >= "A" && c <= "Z") || c === "_";
  const isIdentPart = (c) => isIdentStart(c) || isDigit(c);
  const syntax = (at, msg) => new ExprError(E.SYNTAX, msg, at);

  function lex(src) {
    const out = [];
    const n = src.length;
    let i = 0;
    while (i < n) {
      const c = src[i];
      if (c === " " || c === "\t" || c === "\r" || c === "\n") { i++; continue; }
      const start = i;
      if (isDigit(c)) {
        while (i < n && isDigit(src[i])) i++;
        let frac = false;
        if (src[i] === "." && i + 1 < n && isDigit(src[i + 1])) {
          frac = true;
          i++;
          while (i < n && isDigit(src[i])) i++;
        }
        const text = src.slice(start, i);
        if (frac) {
          const f = Number(text);
          if (!Number.isFinite(f)) throw syntax(start, "number out of range");
          out.push({ k: "float", v: f, s: start });
        } else {
          const b = BigInt(text);
          if (b > 9223372036854775807n) throw syntax(start, "number out of range");
          // Beyond 2^53 a JS number is no longer exact; the contract only promises exactness below it.
          if (b > BigInt(SAFE)) throw syntax(start, "integer beyond 2^53 is not previewed");
          out.push({ k: "int", v: Number(b), s: start });
        }
        continue;
      }
      if (isIdentStart(c)) {
        while (i < n && isIdentPart(src[i])) i++;
        out.push({ k: "id", v: src.slice(start, i), s: start });
        continue;
      }
      if (c === "'") {
        i++;
        let text = "";
        for (;;) {
          if (i >= n) throw syntax(start, "unterminated string");
          const ch = src[i++];
          if (ch === "'") break;
          if (ch === "\\") {
            const e = src[i];
            if (e === "'" || e === "\\") { text += e; i++; } else throw syntax(i - 1, "only \\' and \\\\ are escapes");
          } else text += ch;
        }
        out.push({ k: "str", v: text, s: start });
        continue;
      }
      const two = src.slice(i, i + 2);
      if (TWO.includes(two)) { out.push({ k: "op", v: two, s: start }); i += 2; continue; }
      if (ONE.includes(c)) { out.push({ k: "op", v: c, s: start }); i++; continue; }
      throw syntax(i, "unexpected character " + JSON.stringify(c));
    }
    return out;
  }

  // name -> [fewest, most] arguments (null: no upper bound)
  const FUNCS = {
    "fmt.int": [1, 1], "fmt.signed": [1, 1], "fmt.fixed": [2, 2], "fmt.pct": [1, 1], "fmt.mmss": [1, 1],
    "loc": [1, null], "len": [1, 1], "min": [2, null], "max": [2, null], "clamp": [3, 3],
    "round": [1, 1], "floor": [1, 1], "ceil": [1, 1], "abs": [1, 1],
  };

  // Syntax tree nodes carry their depth `d` (a leaf is 1), so no pass recurses over a deep tree.
  const node = (o, ...kids) => { o.d = 1 + kids.reduce((m, k) => Math.max(m, k.d), 0); return o; };

  class Parser {
    constructor(src) {
      this.src = src;
      this.toks = lex(src);
      this.pos = 0;
      this.depth = 0;
    }
    peek() { return this.pos < this.toks.length ? this.toks[this.pos] : null; }
    here() { const t = this.peek(); return t ? t.s : this.src.length; }
    isOp(op) { const t = this.peek(); return !!t && t.k === "op" && t.v === op; }
    eatOp(op) { if (this.isOp(op)) { this.pos++; return true; } return false; }
    expectOp(op) { if (!this.eatOp(op)) throw syntax(this.here(), "expected `" + op + "`"); }
    enter() { if (++this.depth > MAX_DEPTH) throw syntax(this.here(), "expression nested too deeply"); }
    checked(n) { if (n.d > MAX_DEPTH) throw syntax(0, "expression nested too deeply"); return n; }

    expr() {
      this.enter();
      const cond = this.or();
      let out = cond;
      if (this.eatOp("?")) {
        const then = this.expr();
        this.expectOp(":");
        const other = this.expr();
        out = this.checked(node({ t: "cond", c: cond, a: then, b: other }, cond, then, other));
      }
      this.depth--;
      return out;
    }
    or() {
      let l = this.and();
      while (this.eatOp("||")) { const r = this.and(); l = this.checked(node({ t: "bin", op: "||", a: l, b: r }, l, r)); }
      return l;
    }
    and() {
      let l = this.cmp();
      while (this.eatOp("&&")) { const r = this.cmp(); l = this.checked(node({ t: "bin", op: "&&", a: l, b: r }, l, r)); }
      return l;
    }
    cmp() {
      const l = this.sum();
      const t = this.peek();
      if (!t || t.k !== "op" || !["==", "!=", "<", "<=", ">", ">="].includes(t.v)) return l;
      this.pos++;
      const r = this.sum();
      const t2 = this.peek();
      if (t2 && t2.k === "op" && ["==", "!=", "<", "<=", ">", ">="].includes(t2.v)) throw syntax(this.here(), "comparisons cannot be chained");
      return this.checked(node({ t: "bin", op: t.v, a: l, b: r }, l, r));
    }
    sum() {
      let l = this.prod();
      for (;;) {
        const t = this.peek();
        if (!t || t.k !== "op" || (t.v !== "+" && t.v !== "-")) return l;
        this.pos++;
        const r = this.prod();
        l = this.checked(node({ t: "bin", op: t.v, a: l, b: r }, l, r));
      }
    }
    prod() {
      let l = this.unary();
      for (;;) {
        const t = this.peek();
        if (!t || t.k !== "op" || (t.v !== "*" && t.v !== "/" && t.v !== "%")) return l;
        this.pos++;
        const r = this.unary();
        l = this.checked(node({ t: "bin", op: t.v, a: l, b: r }, l, r));
      }
    }
    unary() {
      const t = this.peek();
      if (!t || t.k !== "op" || (t.v !== "!" && t.v !== "-")) return this.atom();
      this.enter();
      this.pos++;
      const inner = this.unary();
      this.depth--;
      return this.checked(node({ t: "un", op: t.v, a: inner }, inner));
    }
    atom() {
      const at = this.here();
      const t = this.peek();
      if (!t) throw syntax(at, "the expression ends too early");
      this.pos++;
      switch (t.k) {
        case "int": return node({ t: "int", v: t.v });
        case "float": return node({ t: "float", v: t.v });
        case "str": return node({ t: "str", v: t.v });
        case "op":
          if (t.v === "(") { const inner = this.expr(); this.expectOp(")"); return inner; }
          throw syntax(at, "unexpected `" + t.v + "`");
        default:
          if (t.v === "true") return node({ t: "bool", v: true });
          if (t.v === "false") return node({ t: "bool", v: false });
          if (t.v === "null") return node({ t: "null" });
          return this.pathOrCall(t.v, at);
      }
    }
    pathOrCall(first, at) {
      const segs = [first];
      while (this.isOp(".")) {
        this.pos++;
        const t = this.peek();
        if (!t || t.k !== "id") throw syntax(this.here(), "expected a name after `.`");
        this.pos++;
        segs.push(t.v);
      }
      if (!this.eatOp("(")) {
        if (segs.some((s) => s === "true" || s === "false" || s === "null")) throw syntax(at, "a keyword cannot be part of a path");
        return node({ t: "path", segs });
      }
      const name = segs.join(".");
      if (!Object.prototype.hasOwnProperty.call(FUNCS, name)) throw new ExprError(E.UNKNOWN_FUNC, "unknown function `" + name + "`", at);
      const args = [];
      if (!this.eatOp(")")) {
        for (;;) {
          args.push(this.expr());
          if (this.eatOp(",")) continue;
          this.expectOp(")");
          break;
        }
      }
      const [lo, hi] = FUNCS[name];
      if (args.length < lo || (hi !== null && args.length > hi)) {
        throw new ExprError(E.ARGC, "`" + name + "` takes " + (hi === lo ? lo : hi === null ? "at least " + lo : lo + " to " + hi) + " arguments, not " + args.length, at);
      }
      return this.checked(node({ t: "call", f: name, args }, ...args));
    }
  }

  function parseExpr(src) {
    const p = new Parser(src);
    if (p.toks.length === 0) throw syntax(0, "empty expression");
    const e = p.expr();
    if (p.pos < p.toks.length) throw syntax(p.toks[p.pos].s, "unexpected token after the expression");
    if (e.d > MAX_DEPTH) throw syntax(0, "expression nested too deeply");
    return e;
  }

  /** The `}}` that closes an insert starting at `from`, skipping string literals; -1 when there is none. */
  function findClose(src, from) {
    let inStr = false;
    for (let i = from; i < src.length; i++) {
      const c = src[i];
      if (c === "\\" && inStr) i++;
      else if (c === "'") inStr = !inStr;
      else if (c === "}" && !inStr && src[i + 1] === "}") return i;
    }
    return -1;
  }

  /** Text with {{ expr }} inserts -> { segs: [{ lit } | { expr }] }. A `}}` outside an insert is text. */
  function parseTemplate(src) {
    const segs = [];
    let lit = "";
    let i = 0;
    while (i < src.length) {
      const open = src.indexOf("{{", i);
      if (open < 0) { lit += src.slice(i); break; }
      lit += src.slice(i, open);
      const body = open + 2;
      const close = findClose(src, body);
      if (close < 0) throw new ExprError(E.UNCLOSED, "`{{` has no closing `}}`", open);
      if (lit) { segs.push({ lit }); lit = ""; }
      let expr;
      try { expr = parseExpr(src.slice(body, close)); } catch (e) { if (e instanceof ExprError && e.at !== null) e.at += body; throw e; }
      segs.push({ expr });
      i = close + 2;
    }
    if (lit) segs.push({ lit });
    return { segs };
  }

  // ------------------------------------------------------------------------------------------------
  // Evaluator. Evaluation never throws: an operation without a result gives null; division by zero,
  // integer overflow and a non-finite result also tell host.warn(). Both sides of an arithmetic or
  // comparison operator are evaluated left to right; &&, || and ?: evaluate only what they need.
  // host = { path(segs) -> Value, loc(key, args) -> string, fmtInt(n) -> string, warn(message) }.
  // ------------------------------------------------------------------------------------------------

  const isNum = (v) => typeof v === "number" || v instanceof Flt;
  const numF = (v) => (typeof v === "number" ? v : v.v);

  function truthy(v) {
    if (v === null || v === undefined) return false;
    if (typeof v === "boolean") return v;
    if (typeof v === "number") return v !== 0;
    if (typeof v === "string") return v !== "";
    if (v instanceof Flt) return v.v !== 0;
    if (v instanceof Lst) return v.n !== 0;
    return true; // Col, Struct
  }

  const roundAway = (f) => (f < 0 ? -Math.round(-f) : Math.round(f)) + 0; // half away from zero
  function toInt(f) {
    if (Number.isNaN(f)) return 0;
    const r = roundAway(f);
    return r > SAFE ? SAFE : r < -SAFE ? -SAFE : r;
  }

  function codePointCompare(a, b) {
    // Rust compares UTF-8 bytes, which orders by code point; JS `<` orders by UTF-16 unit.
    const x = Array.from(a);
    const y = Array.from(b);
    const n = Math.min(x.length, y.length);
    for (let i = 0; i < n; i++) {
      const d = x[i].codePointAt(0) - y[i].codePointAt(0);
      if (d !== 0) return d < 0 ? -1 : 1;
    }
    return x.length === y.length ? 0 : x.length < y.length ? -1 : 1;
  }

  function equal(l, r) {
    if (l === null && r === null) return true;
    if (typeof l === "boolean" && typeof r === "boolean") return l === r;
    if (isNum(l) && isNum(r)) return numF(l) === numF(r);
    if (typeof l === "string" && typeof r === "string") return l === r;
    if (l instanceof Col && r instanceof Col) return l.c === r.c;
    return false;
  }

  /** -1, 0, 1, or null when the operands cannot be ordered. */
  function compare(l, r) {
    if (typeof l === "string" && typeof r === "string") return codePointCompare(l, r);
    if (!isNum(l) || !isNum(r)) return null;
    const a = numF(l);
    const b = numF(r);
    return a < b ? -1 : a > b ? 1 : a === b ? 0 : null;
  }

  function arithmetic(host, op, a, b) {
    if (typeof a === "number" && typeof b === "number" && op !== "/") {
      let r;
      if (op === "+") r = a + b;
      else if (op === "-") r = a - b;
      else if (op === "*") r = a * b;
      else {
        if (b === 0) { host.warn("division by zero"); return null; }
        r = a % b;
      }
      if (!Number.isSafeInteger(r)) { host.warn("integer overflow"); return null; }
      return r + 0;
    }
    const x = numF(a);
    const y = numF(b);
    if ((op === "/" || op === "%") && y === 0) { host.warn("division by zero"); return null; }
    const r = op === "+" ? x + y : op === "-" ? x - y : op === "*" ? x * y : op === "/" ? x / y : x % y;
    if (!Number.isFinite(r)) { host.warn("a result is not a finite number"); return null; }
    return new Flt(r);
  }

  function extreme(nums, want) {
    const allInt = nums.every((n) => typeof n === "number");
    let best = nums[0];
    for (let i = 1; i < nums.length; i++) {
      const a = numF(nums[i]);
      const b = numF(best);
      if ((want < 0 && a < b) || (want > 0 && a > b)) best = nums[i];
    }
    return allInt ? best : new Flt(numF(best));
  }

  function groupDigits(digits) {
    let out = "";
    for (let i = 0; i < digits.length; i++) {
      if (i > 0 && (digits.length - i) % 3 === 0) out += ",";
      out += digits[i];
    }
    return out;
  }
  /** An integer the way the preview language (en_US) groups digits: 1,234. */
  function groupInt(n) { return (n < 0 ? "-" : "") + groupDigits(String(Math.abs(n))); }

  /** A number with exactly `digits` decimals, halves rounded away from zero. */
  function fmtFixed(x, digits) {
    const scale = Math.pow(10, digits);
    const scaled = roundAway(Math.abs(x) * scale);
    if (!Number.isFinite(scaled) || scaled >= 9.0e15) {
      if (Math.abs(x) < 1e21) return x.toFixed(digits);
      return (x < 0 ? "-" : "") + BigInt(Math.abs(x)).toString() + (digits ? "." + "0".repeat(digits) : "");
    }
    const unit = Math.pow(10, digits);
    const sign = x < 0 && scaled !== 0 ? "-" : "";
    if (digits === 0) return sign + String(scaled);
    return sign + String(Math.floor(scaled / unit)) + "." + String(scaled % unit).padStart(digits, "0");
  }

  /** A float in a template: up to four decimals, trailing zeros dropped, no exponent. */
  function fmtFloat(x) {
    let s = fmtFixed(x, 4);
    if (s.includes(".")) {
      s = s.replace(/0+$/, "");
      if (s.endsWith(".")) s = s.slice(0, -1);
    }
    return s;
  }

  function hex(n, width) { return n.toString(16).padStart(width, "0"); }

  /** A value as the text of an insert: null is empty, a float follows fmtFloat. */
  function renderValue(v) {
    if (v === null || v === undefined || v instanceof Lst || v instanceof Struct) return "";
    if (typeof v === "boolean") return v ? "true" : "false";
    if (typeof v === "number") return String(v);
    if (v instanceof Flt) return fmtFloat(v.v);
    if (typeof v === "string") return v;
    if (v instanceof Col) return (v.c & 0xff) === 0xff ? "#" + hex(v.c >>> 8, 6) : "#" + hex(v.c, 8);
    return "";
  }

  function callFunc(host, f, args) {
    if (f === "loc") {
      const key = typeof args[0] === "string" ? args[0] : "";
      return host.loc(key, args.slice(1));
    }
    if (f === "len") return args[0] instanceof Lst ? args[0].n : null;
    for (const a of args) if (!isNum(a)) return null;
    switch (f) {
      case "fmt.int": return host.fmtInt(toInt(numF(args[0])));
      case "fmt.signed": { const n = toInt(numF(args[0])); const t = host.fmtInt(n); return n > 0 ? "+" + t : t; }
      case "fmt.fixed": {
        if (typeof args[1] !== "number") return null;
        return fmtFixed(numF(args[0]), Math.min(6, Math.max(0, args[1])));
      }
      case "fmt.pct": return toInt(numF(args[0]) * 100) + "%";
      case "fmt.mmss": {
        const x = numF(args[0]);
        const total = Math.abs(Math.trunc(x));
        return (x <= -1 ? "-" : "") + Math.floor(total / 60) + ":" + String(total % 60).padStart(2, "0");
      }
      case "min": return extreme(args, -1);
      case "max": return extreme(args, 1);
      case "clamp": {
        const raised = extreme([args[0], args[1]], 1);
        return extreme([raised, args[2]], -1);
      }
      case "round": return toInt(numF(args[0]));
      case "floor": return Math.max(-SAFE, Math.min(SAFE, Math.floor(numF(args[0])))) + 0;
      case "ceil": return Math.max(-SAFE, Math.min(SAFE, Math.ceil(numF(args[0])))) + 0;
      case "abs": return typeof args[0] === "number" ? Math.abs(args[0]) : new Flt(Math.abs(args[0].v));
      default: return null;
    }
  }

  function evalExpr(e, host) {
    switch (e.t) {
      case "null": return null;
      case "bool": case "int": case "str": return e.v;
      case "float": return new Flt(e.v);
      case "path": { const v = host.path(e.segs); return v === undefined ? null : v; }
      case "un": {
        const v = evalExpr(e.a, host);
        if (e.op === "!") return !truthy(v);
        if (typeof v === "number") return -v + 0;
        if (v instanceof Flt) return new Flt(-v.v);
        return null;
      }
      case "bin": {
        if (e.op === "&&") return truthy(evalExpr(e.a, host)) && truthy(evalExpr(e.b, host));
        if (e.op === "||") return truthy(evalExpr(e.a, host)) || truthy(evalExpr(e.b, host));
        const l = evalExpr(e.a, host);
        const r = evalExpr(e.b, host);
        switch (e.op) {
          case "==": return equal(l, r);
          case "!=": return !equal(l, r);
          case "<": return compare(l, r) === -1;
          case "<=": { const c = compare(l, r); return c === -1 || c === 0; }
          case ">": return compare(l, r) === 1;
          case ">=": { const c = compare(l, r); return c === 1 || c === 0; }
          default: return isNum(l) && isNum(r) ? arithmetic(host, e.op, l, r) : null;
        }
      }
      case "cond": return truthy(evalExpr(e.c, host)) ? evalExpr(e.a, host) : evalExpr(e.b, host);
      case "call": return callFunc(host, e.f, e.args.map((a) => evalExpr(a, host)));
      default: return null;
    }
  }

  function renderTemplate(tpl, host) {
    let out = "";
    for (const s of tpl.segs) out += s.expr ? renderValue(evalExpr(s.expr, host)) : s.lit;
    return out;
  }

  // ------------------------------------------------------------------------------------------------
  // JSON: floats written with a fraction or exponent stay floats (JSON.parse alone cannot tell 1 from
  // 1.0; the reviver's source text can, in current Chromium and Node).
  // ------------------------------------------------------------------------------------------------

  function parseJson(text) {
    return JSON.parse(text, function (_key, value, ctx) {
      if (typeof value === "number" && ctx && typeof ctx.source === "string" && /[.eE]/.test(ctx.source)) return new Flt(value);
      return value;
    });
  }

  const isPlainObject = (x) => x !== null && typeof x === "object" && !Array.isArray(x) && !(x instanceof Flt);
  /** A model text: { k: "<key>", a: [args] }. */
  const isTextObject = (o) => typeof o.k === "string" && (o.a === undefined || Array.isArray(o.a)) && Object.keys(o).every((k) => k === "k" || k === "a");

  /** JSON data -> a Value. `locText(key, args)` renders model texts; without it they are plain records. */
  function fromJson(x, locText) {
    if (x === null || x === undefined) return null;
    if (x instanceof Flt || x instanceof Col || x instanceof Lst || x instanceof Struct) return x;
    if (typeof x === "boolean" || typeof x === "string") return x;
    if (typeof x === "number") return Number.isSafeInteger(x) ? x : Number.isInteger(x) ? null : new Flt(x);
    if (Array.isArray(x)) return new Lst(x.length, x);
    if (typeof x === "object") {
      if (typeof x.$color === "string") {
        const d = x.$color.replace(/^#/, "");
        const v = parseInt(d, 16);
        return new Col(d.length === 6 ? ((v << 8) | 0xff) >>> 0 : v);
      }
      if (locText && isTextObject(x)) return locText(x.k, (x.a || []).map((a) => fromJson(a, locText)));
      return new Struct(x);
    }
    return null;
  }

  // ------------------------------------------------------------------------------------------------
  // Attribute grammars (the ones the preview evaluates; the same rules as the converter's parser).
  // ------------------------------------------------------------------------------------------------

  const isIdent = (s) => /^[A-Za-z_][A-Za-z0-9_]*$/.test(s);
  const isClass = (s) => /^[A-Za-z_-][A-Za-z0-9_-]*$/.test(s);

  /** Splits at `sep` outside string literals, parentheses and {{ }} inserts: [[offset, piece]]. */
  function splitTop(src, sep) {
    const out = [];
    let start = 0;
    let depth = 0;
    let inStr = false;
    for (let i = 0; i < src.length; i++) {
      const c = src[i];
      if (inStr) {
        if (c === "\\") i++;
        else if (c === "'") inStr = false;
      } else if (src.startsWith("{{", i)) {
        const close = findClose(src, i + 2);
        if (close >= 0) i = close + 1;
      } else if (c === "'") inStr = true;
      else if (c === "(") depth++;
      else if (c === ")") depth = Math.max(0, depth - 1);
      else if (c === sep && depth === 0) { out.push([start, src.slice(start, i)]); start = i + 1; }
    }
    out.push([start, src.slice(Math.min(start, src.length))]);
    return out;
  }

  /** Exactly one {{ expr }} (spaces around it are fine) -> the expression, or throws ExprError. */
  function parseSingleExpr(value) {
    const t = parseTemplate(value.trim());
    if (t.segs.length === 1 && t.segs[0].expr) return t.segs[0].expr;
    throw new ExprError(E.ONE_EXPR, "the value must be exactly one {{ }} insert");
  }

  /** `[idx,] item:{{ list }}` -> { item, index, list }. */
  function parseForHead(value) {
    const open = value.indexOf("{{");
    const bad = () => new ExprError(E.FOR, "the form is `[index,] item:{{ list }}`");
    if (open < 0) throw bad();
    const head = value.slice(0, open).trim();
    if (!head.endsWith(":")) throw bad();
    const names = head.slice(0, -1).split(",").map((s) => s.trim());
    if (names.length < 1 || names.length > 2 || !names.every(isIdent)) throw new ExprError(E.FOR, "one or two variable names, then a colon");
    const list = parseSingleExpr(value.slice(open));
    return names.length === 1 ? { item: names[0], index: null, list } : { item: names[1], index: names[0], list };
  }

  /** `cls:{{ cond }}[; cls2:{{ cond }}]` -> [{ cls, cond }]. */
  function parseClassToggle(value) {
    const out = [];
    for (const [, part] of splitTop(value, ";")) {
      if (part.trim() === "") continue;
      const colon = part.indexOf(":");
      if (colon < 0) throw new ExprError(E.CLASS_TOGGLE, "the form is `class:{{ cond }}`");
      const cls = part.slice(0, colon).trim();
      if (!isClass(cls) || out.some((c) => c.cls === cls)) throw new ExprError(E.CLASS_TOGGLE, "a class name, once");
      out.push({ cls, cond: parseSingleExpr(part.slice(colon + 1)) });
    }
    if (out.length === 0) throw new ExprError(E.CLASS_TOGGLE, "the list is empty");
    return out;
  }

  /** `{{ a, b }}[, {{ c }}]` -> [expr]. */
  function parseL10nArgs(value) {
    const args = [];
    let rest = value;
    for (;;) {
      const body = rest.trimStart();
      if (!body.startsWith("{{")) throw new ExprError(E.ONE_EXPR, "the value is a list of {{ }} inserts");
      const close = findClose(body, 2);
      if (close < 0) throw new ExprError(E.ONE_EXPR, "an insert is not closed");
      for (const [, part] of splitTop(body.slice(2, close), ",")) args.push(parseExpr(part));
      const tail = body.slice(close + 2).trimStart();
      if (tail === "") break;
      if (!tail.startsWith(",")) throw new ExprError(E.ONE_EXPR, "the inserts are separated by commas");
      rest = tail.slice(1);
    }
    return args;
  }

  /**
   * The bindings of one element from its attributes ([[name, value]] in document order).
   * Returns { value, cond, forSpec, classToggle, styles, src, disabled, l10n, l10nArgs, errors } where
   * errors = [{ code, attr, message }]. Attributes the preview does not evaluate are ignored.
   */
  function parseBindings(attrs) {
    const out = { value: null, cond: null, forSpec: null, classToggle: [], styles: [], src: null, disabled: null, l10n: null, l10nArgs: null, errors: [] };
    const seen = new Set();
    const raw = {};
    const take = (attr, fn) => {
      try { return fn(); } catch (e) {
        if (!(e instanceof ExprError)) throw e;
        out.errors.push({ code: e.code, attr, message: e.message });
        return null;
      }
    };
    for (const [name, value] of attrs) {
      if (seen.has(name)) { out.errors.push({ code: E.DUP_ATTR, attr: name, message: "the attribute is written twice" }); continue; }
      seen.add(name);
      raw[name] = value;
      switch (name) {
        case "data-bind-value": out.value = take(name, () => parseTemplate(value)); break;
        case "data-bind-if": out.cond = take(name, () => parseSingleExpr(value)); break;
        case "data-bind-class-toggle": out.classToggle = take(name, () => parseClassToggle(value)) || []; break;
        case "data-bind-src": out.src = take(name, () => parseSingleExpr(value)); break;
        case "data-bind-disabled": out.disabled = take(name, () => parseSingleExpr(value)); break;
        case "data-l10n": out.l10n = value; break;
        case "data-l10n-args": out.l10nArgs = take(name, () => parseL10nArgs(value)); break;
        default:
          if (name.startsWith("data-bind-style-")) {
            const tpl = take(name, () => parseTemplate(value));
            if (tpl) out.styles.push({ prop: name.slice("data-bind-style-".length), tpl });
          }
      }
    }
    if ("data-bind-for" in raw) {
      const spec = take("data-bind-for", () => parseForHead(raw["data-bind-for"]));
      if (spec) {
        spec.key = "data-bind-key" in raw ? take("data-bind-key", () => parseSingleExpr(raw["data-bind-key"])) : null;
        spec.sort = "data-bind-sort" in raw ? take("data-bind-sort", () => parseSingleExpr(raw["data-bind-sort"])) : null;
        spec.sortDesc = "data-bind-sort-desc" in raw ? take("data-bind-sort-desc", () => parseSingleExpr(raw["data-bind-sort-desc"])) : null;
        spec.filter = "data-bind-filter" in raw ? take("data-bind-filter", () => parseSingleExpr(raw["data-bind-filter"])) : null;
        out.forSpec = spec;
        if (out.cond) out.errors.push({ code: E.IF_FOR, attr: "data-bind-if", message: "put the condition on a parent element" });
      }
    }
    if (out.value && out.l10n !== null) out.errors.push({ code: E.TEXT_SOURCE, attr: "data-bind-value", message: "an element has one source of text" });
    return out;
  }

  /** Escapes a loop key for the `/`-joined instance path. */
  const escKey = (k) => String(k).replace(/%/g, "%25").replace(/\//g, "%2F");

  /**
   * The runtime's label of a list key (`ui2_model::Key` shown as `hi:lo`): a `key<..>` value is `"hi:lo"` or
   * the 64-bit number; an integer key is its 32-bit pattern; an enum key its variant index; a string key the
   * FNV-1a 64 hash of its UTF-8 bytes. `info` = { field, kind: key|int|uint|enum|str, variants? }. Null when
   * the value does not fit the kind.
   */
  function keyLabel(v, info) {
    const split = (n) => `${n >> 32n}:${n & 0xffffffffn}`;
    switch (info.kind) {
      case "key":
        if (typeof v === "string") {
          const m = /^(\d+):(\d+)$/.exec(v);
          return m ? `${Number(m[1])}:${Number(m[2])}` : null;
        }
        return Number.isSafeInteger(v) && v >= 0 ? split(BigInt(v)) : null;
      case "int":
        return Number.isInteger(v) ? `0:${v >>> 0}` : null;
      case "uint":
        return Number.isInteger(v) && v >= 0 ? `0:${v}` : null;
      case "enum": {
        const i = (info.variants || []).indexOf(v);
        return i >= 0 ? `0:${i}` : null;
      }
      case "str": {
        if (typeof v !== "string") return null;
        let h = 0xcbf29ce484222325n;
        for (const b of new TextEncoder().encode(v)) h = BigInt.asUintN(64, (h ^ BigInt(b)) * 0x100000001b3n);
        return split(h);
      }
      default:
        return null;
    }
  }

  const sortRank = (v) => (isNum(v) ? 0 : typeof v === "string" ? 1 : 2);

  /**
   * Which items of a `for` are drawn, in which order, under which key. `ev` supplies evaluation:
   *   ev.item(expr, item, position) -> Value   an expression with the loop variables bound;
   *   ev.outer(expr) -> Value                  an expression outside the loop.
   * Filter first, then the stable sort (numbers, then strings by code point, null last; descending
   * reverses numbers and strings only), then the key: the list key of the model schema when the list is a
   * model list (`keyInfo`, labelled as the runtime labels instance keys), else data-bind-key, else the item's
   * `id`, else its position. Returns [{ item, key }].
   */
  function planFor(items, spec, ev, locText, keyInfo) {
    let rows = items.map((item, i) => ({ item, i }));
    if (spec.filter) rows = rows.filter((r) => truthy(ev.item(spec.filter, r.item, r.i)));
    if (spec.sort) {
      const desc = spec.sortDesc ? truthy(ev.outer(spec.sortDesc)) : false;
      rows.forEach((r) => { r.k = ev.item(spec.sort, r.item, r.i); });
      rows.sort((a, b) => {
        const ra = sortRank(a.k);
        const rb = sortRank(b.k);
        if (ra !== rb) return ra - rb;
        if (ra === 2) return a.i - b.i;
        let c = ra === 0 ? Math.sign(numF(a.k) - numF(b.k)) : codePointCompare(a.k, b.k);
        if (desc) c = -c;
        return c || a.i - b.i;
      });
    }
    return rows.map((r, pos) => {
      let key;
      const label = keyInfo && isPlainObject(r.item) ? keyLabel(r.item[keyInfo.field], keyInfo) : null;
      if (label !== null) key = label;
      else if (spec.key) key = renderValue(ev.item(spec.key, r.item, pos));
      else if (isPlainObject(r.item) && r.item.id !== undefined && r.item.id !== null) key = renderValue(fromJson(r.item.id, locText));
      else key = String(pos);
      return { item: r.item, key };
    });
  }

  // ------------------------------------------------------------------------------------------------
  // The tables of the skin: a TOML subset (flat `"key" = "value"`), and the text engine subset.
  // ------------------------------------------------------------------------------------------------

  /**
   * Flat TOML: `key = "value"` with bare, "quoted" or 'quoted' keys, basic and literal strings, comments,
   * and [section] headers (their name prefixes the keys, "section.key"). Returns { table, errors } with
   * errors = [{ line, message }]; the lines that cannot be read are skipped.
   */
  function parseToml(text) {
    const table = {};
    const errors = [];
    let prefix = "";
    const lines = text.replace(/^﻿/, "").split(/\r\n|\n|\r/);
    const str = (s, i) => {
      // reads a string starting at s[i] (a quote); returns [value, nextIndex] or null
      const q = s[i];
      let out = "";
      for (let j = i + 1; j < s.length; j++) {
        const c = s[j];
        if (c === q) return [out, j + 1];
        if (q === '"' && c === "\\") {
          const e = s[++j];
          const map = { n: "\n", t: "\t", r: "\r", b: "\b", f: "\f", '"': '"', "\\": "\\" };
          if (e in map) out += map[e];
          else if (e === "u" || e === "U") {
            const len = e === "u" ? 4 : 8;
            const cp = parseInt(s.slice(j + 1, j + 1 + len), 16);
            if (!Number.isFinite(cp)) return null;
            out += String.fromCodePoint(cp);
            j += len;
          } else return null;
        } else out += c;
      }
      return null;
    };
    lines.forEach((raw, n) => {
      const line = raw.trim();
      if (line === "" || line[0] === "#") return;
      const fail = (message) => errors.push({ line: n + 1, message });
      if (line[0] === "[") {
        const m = /^\[\s*([A-Za-z0-9_.-]+)\s*\]\s*(#.*)?$/.exec(line);
        if (m) prefix = m[1] + "."; else fail("this table header is not read");
        return;
      }
      let key;
      let i;
      if (line[0] === '"' || line[0] === "'") {
        const r = str(line, 0);
        if (!r) return fail("bad quoted key");
        [key, i] = r;
      } else {
        const m = /^[A-Za-z0-9_.-]+/.exec(line);
        if (!m) return fail("a key is expected");
        key = m[0];
        i = key.length;
      }
      while (line[i] === " " || line[i] === "\t") i++;
      if (line[i] !== "=") return fail("`=` is expected after the key");
      i++;
      while (line[i] === " " || line[i] === "\t") i++;
      if (line[i] !== '"' && line[i] !== "'") return fail("only string values are read");
      if (line.startsWith('"""', i) || line.startsWith("'''", i)) return fail("multi-line strings are not read");
      const r = str(line, i);
      if (!r) return fail("bad string value");
      const rest = line.slice(r[1]).trim();
      if (rest !== "" && rest[0] !== "#") return fail("text after the value");
      table[prefix + key] = r[0];
    });
    return { table, errors };
  }

  /**
   * The name a key shows when the skin has no text `key.<code>`: the letter of KeyA..KeyZ, the digit of
   * Digit0..Digit9, otherwise the code itself (the runtime's rule).
   */
  function defaultKeyName(code) {
    const m = /^(?:Key|Digit)(.)$/u.exec(code);
    return m ? m[1] : code;
  }

  /**
   * The text of a hotkey label for a `data-hotkey` value (`Ctrl+Shift+KeyB`): the modifiers, then the key,
   * joined by `+`. `name(key, fallback)` gives the skin's text `key.<key>` or the fallback.
   */
  function hotkeyLabel(hotkey, name) {
    const parts = String(hotkey).split("+");
    const code = parts.pop();
    let out = "";
    for (const m of ["Ctrl", "Alt", "Shift"]) if (parts.includes(m)) out += name(m, m) + "+";
    return out + name(code, defaultKeyName(code));
  }

  /**
   * The skin state of `skin.toml`: the `[state]` table (and `[state.<group>]` tables) with booleans,
   * integers, floats (Flt, as parseJson reads a number with a fraction), strings and inline tables. A dotted
   * key, bare or quoted, is a path of groups, as the converter flattens it. Returns { state, errors } with
   * state a nested object and errors = [{ line, message }]; other tables are skipped.
   */
  function parseSkinState(text) {
    const state = {};
    const errors = [];
    let base = null; // the path of the current table below `state`, or null outside it
    const fail = (line, message) => { const e = new Error(message); e.line = line; throw e; };
    text.replace(/^﻿/, "").split(/\r\n|\n|\r/).forEach((raw, n) => {
      const line = raw.trim();
      if (line === "" || line[0] === "#") return;
      if (line[0] === "[") {
        const m = /^\[\s*([A-Za-z0-9_.-]+)\s*\]\s*(#.*)?$/.exec(line);
        const name = m ? m[1] : "";
        base = name === "state" || name.startsWith("state.") ? name.split(".").slice(1) : null;
        return;
      }
      if (base === null) return;
      const p = { s: line, i: 0, line: n + 1 };
      try {
        const key = readKey(p);
        ws(p);
        if (p.s[p.i] !== "=") fail(p.line, "`=` is expected after the key");
        p.i++;
        const value = readValue(p);
        ws(p);
        if (p.i < p.s.length && p.s[p.i] !== "#") fail(p.line, "text after the value");
        put(state, base.concat(key), value, p.line);
      } catch (e) {
        errors.push({ line: n + 1, message: e.message });
      }
    });
    return { state, errors };

    function ws(p) { while (p.s[p.i] === " " || p.s[p.i] === "\t") p.i++; }
    function quoted(p) {
      const q = p.s[p.i];
      let out = "";
      for (let j = p.i + 1; j < p.s.length; j++) {
        const c = p.s[j];
        if (c === q) { p.i = j + 1; return out; }
        if (q === '"' && c === "\\") {
          const e = p.s[++j];
          const map = { n: "\n", t: "\t", r: "\r", b: "\b", f: "\f", '"': '"', "\\": "\\" };
          if (e in map) out += map[e];
          else if (e === "u" || e === "U") {
            const len = e === "u" ? 4 : 8;
            const cp = parseInt(p.s.slice(j + 1, j + 1 + len), 16);
            if (!Number.isFinite(cp)) fail(p.line, "bad escape");
            out += String.fromCodePoint(cp);
            j += len;
          } else fail(p.line, "bad escape");
        } else out += c;
      }
      return fail(p.line, "unclosed string");
    }
    function readKey(p) {
      const segs = [];
      for (;;) {
        ws(p);
        const c = p.s[p.i];
        if (c === '"' || c === "'") segs.push(...quoted(p).split("."));
        else {
          const m = /^[A-Za-z0-9_-]+/.exec(p.s.slice(p.i));
          if (!m) fail(p.line, "a key is expected");
          segs.push(m[0]);
          p.i += m[0].length;
        }
        ws(p);
        if (p.s[p.i] !== ".") return segs;
        p.i++;
      }
    }
    function readValue(p) {
      ws(p);
      const c = p.s[p.i];
      if (c === '"' || c === "'") {
        if (p.s.startsWith(c.repeat(3), p.i)) fail(p.line, "multi-line strings are not read");
        return quoted(p);
      }
      if (c === "{") {
        p.i++;
        const obj = {};
        ws(p);
        if (p.s[p.i] === "}") { p.i++; return obj; }
        for (;;) {
          const key = readKey(p);
          if (p.s[p.i] !== "=") fail(p.line, "`=` is expected after the key");
          p.i++;
          put(obj, key, readValue(p), p.line);
          ws(p);
          if (p.s[p.i] === ",") { p.i++; continue; }
          if (p.s[p.i] === "}") { p.i++; return obj; }
          fail(p.line, "`,` or `}` is expected in an inline table");
        }
      }
      const rest = p.s.slice(p.i);
      const b = /^(true|false)(?![A-Za-z0-9_])/.exec(rest);
      if (b) { p.i += b[0].length; return b[1] === "true"; }
      const m = /^[+-]?\d[\d_]*(\.\d[\d_]*)?([eE][+-]?\d[\d_]*)?/.exec(rest);
      if (m) {
        p.i += m[0].length;
        const v = Number(m[0].replace(/_/g, ""));
        return m[1] !== undefined || m[2] !== undefined ? new Flt(v) : v;
      }
      return fail(p.line, "a boolean, number, string or inline table is expected");
    }
    function put(obj, path, value, line) {
      let at = obj;
      for (let k = 0; k < path.length - 1; k++) {
        const seg = path[k];
        if (!Object.prototype.hasOwnProperty.call(at, seg)) at[seg] = {};
        else if (!isPlainObject(at[seg])) fail(line, "state." + path.join(".") + " is a value and a group at once");
        at = at[seg];
      }
      const last = path[path.length - 1];
      if (Object.prototype.hasOwnProperty.call(at, last)) fail(line, "state." + path.join(".") + " is written twice");
      at[last] = value;
    }
  }

  /**
   * The state a sample sees: the skin's defaults with the sample's `state` written over them, as the runtime
   * mounts the skin state and then applies the sample (a group is walked, any other value replaces). Neither
   * argument is changed.
   */
  function mergeState(defaults, state) {
    const out = {};
    for (const [k, v] of Object.entries(defaults || {})) out[k] = isPlainObject(v) ? mergeState(v, {}) : v;
    for (const [k, v] of Object.entries(isPlainObject(state) ? state : {})) {
      out[k] = isPlainObject(v) && isPlainObject(out[k]) ? mergeState(out[k], v) : v;
    }
    return out;
  }

  /** `1,234` for ints, up to three decimals (trailing zeros dropped) for floats: the engine's "#,##0.###". */
  function defaultNumber(v) {
    if (typeof v === "number") return groupInt(v);
    let s = fmtFixed(v.v, 3);
    if (s.includes(".")) s = s.replace(/0+$/, "").replace(/\.$/, "");
    const neg = s.startsWith("-");
    const [int, frac] = (neg ? s.slice(1) : s).split(".");
    return (neg ? "-" : "") + groupDigits(int) + (frac ? "." + frac : "");
  }

  /** Index of the `}` that closes the `{` at `open`, counting braces; -1 if none. */
  function matchBrace(s, open) {
    let depth = 0;
    for (let i = open; i < s.length; i++) {
      if (s[i] === "{") depth++;
      else if (s[i] === "}" && --depth === 0) return i;
    }
    return -1;
  }

  /** The conditional of the engine: `key?value; key?value; other?value;` (first clause that matches). */
  function conditional(spec, matches, render) {
    let pos = 0;
    while (pos < spec.length) {
      const q = spec.indexOf("?", pos);
      if (q < 0) return "";
      const key = spec.slice(pos, q);
      let depth = 0;
      let end = q;
      let found = false;
      for (; end < spec.length; end++) {
        const c = spec[end];
        if (c === ";" && depth === 0) { found = true; break; }
        if (c === "}") depth--;
        else if (c === "{") depth++;
      }
      if (!found) return "";
      if (matches(key) || "other".startsWith(key)) return render(spec.slice(q + 1, end));
      pos = end + 1;
      while (spec[pos] === " " || spec[pos] === "\t") pos++;
    }
    return "";
  }

  /**
   * Renders a text of the skin's tables with positional arguments (Values). Returns { text, unsupported }
   * where unsupported lists the `{...}` groups left as written.
   */
  function locRender(template, args, depth) {
    const unsupported = [];
    const go = (tpl, d) => {
      let out = "";
      for (let i = 0; i < tpl.length; i++) {
        const c = tpl[i];
        if (c !== "{") { out += c; continue; }
        const close = matchBrace(tpl, i);
        if (close < 0) { out += tpl.slice(i); break; }
        const group = tpl.slice(i, close + 1);
        const inner = group.slice(1, -1);
        const m = /^([1-9][0-9]*)([A-Za-z_][A-Za-z0-9_]*)?\s*(?::\s*([A-Za-z]+)[ \t]*([\s\S]*))?$/.exec(inner);
        if (!m || d > 8 || (m[3] !== undefined && m[3] !== "plural")) { unsupported.push(group); out += group; i = close; continue; }
        const arg = args[Number(m[1]) - 1];
        if (m[3] === undefined) {
          out += isNum(arg) ? defaultNumber(arg) : renderValue(arg === undefined ? null : arg);
        } else {
          const n = isNum(arg) ? Math.trunc(numF(arg)) : 0;
          const form = String(n === 1 ? 1 : 2); // English: one form for 1, another for everything else
          out += conditional(m[4].replace(/[ \t\r\n]+$/, ""), (key) => form.startsWith(key), (text) => go(text, d + 1));
        }
        i = close;
      }
      return out;
    };
    const text = go(template, depth || 0);
    return { text, unsupported };
  }

  // ------------------------------------------------------------------------------------------------
  // The pure core, exported for tests and for the page part below.
  // ------------------------------------------------------------------------------------------------

  const core = {
    version: VERSION, MAX_DEPTH, Flt, Col, Lst, Struct, ExprError, E, PV,
    parseExpr, parseTemplate, evalExpr, renderTemplate, renderValue, truthy, fmtFixed, fmtFloat, groupInt,
    parseJson, fromJson, isPlainObject,
    splitTop, parseSingleExpr, parseForHead, parseClassToggle, parseL10nArgs, parseBindings,
    planFor, keyLabel, escKey, parseToml, parseSkinState, mergeState, defaultKeyName, hotkeyLabel, locRender,
    codePointCompare,
  };

  // ------------------------------------------------------------------------------------------------
  // The page part.
  // ------------------------------------------------------------------------------------------------

  const ASSET_PLACEHOLDER = "data:image/svg+xml;charset=utf-8," + encodeURIComponent(
    '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><rect width="16" height="16" rx="3" fill="#fff" fill-opacity=".18"/></svg>');
  const NOT_NODES = new Set(["script", "style", "template", "link", "meta"]);

  function install(win) {
    const doc = win.document;
    // The server writes this script into the page; it is not part of the skin, so it leaves the DOM the converter reads.
    const ownScript = doc.currentScript && /\/__ui2\/preview\.js$/.test(doc.currentScript.src || "") ? doc.currentScript : null;
    const errors = [];
    const seenErrors = new Set();
    let staticErrors = 0;
    let resolveReady;
    const ready = new Promise((r) => { resolveReady = r; });

    const g = win.__UI2_PREVIEW || {};
    const query = new URLSearchParams(win.location.search);
    const cfg = {
      mode: String(g.mode || query.get("ui2mode") || "layout"),
      state: String(g.state || query.get("ui2state") || "hud"),
      lang: String(g.lang || query.get("ui2lang") || "en_US"),
      root: g.root || (win.location.protocol === "file:" ? new URL("./", win.location.href).href : win.location.origin + "/"),
    };
    const rootUrl = cfg.root.endsWith("/") ? cfg.root : cfg.root + "/";

    const api = { version: VERSION, errors, ready, core, get mode() { return cfg.mode; }, get state() { return cfg.state; }, get lang() { return cfg.lang; } };
    win.__ui2preview = api;

    function report(code, level, el, attr, message) {
      const src = el && el.getAttribute ? el.getAttribute("data-ui2-src") : null;
      const nodeId = el && el.getAttribute ? el.getAttribute("data-ui2-node") : null;
      const key = [code, src, nodeId, attr, message].join("\u0000");
      if (seenErrors.has(key)) return;
      seenErrors.add(key);
      errors.push({ code, level, src, node: nodeId, attr: attr || null, message });
    }

    // --- includes and numbering ---

    const includeCache = new Map();
    function fetchText(path) {
      if (!includeCache.has(path)) {
        includeCache.set(path, fetch(new URL(path, rootUrl).href, { cache: "no-store" }).then((r) => {
          if (!r.ok) throw new Error("HTTP " + r.status);
          return r.text();
        }));
      }
      return includeCache.get(path);
    }

    const validInclude = (v) => v !== "" && !v.startsWith("/") && !v.includes("\\") && !v.includes(":") && !v.split("/").some((s) => s === "" || s === "..");

    async function expandIncludes(parent, stack) {
      for (const el of Array.from(parent.querySelectorAll("[data-include]"))) {
        if (!parent.contains(el)) continue; // inside an element that was replaced already
        const path = el.getAttribute("data-include");
        if (!validInclude(path)) { report(E.INCLUDE, "error", el, "data-include", "the path is relative, uses `/` and stays inside the skin"); continue; }
        if (stack.includes(path)) { report(PV.INCLUDE_CYCLE, "error", el, "data-include", "include cycle: " + stack.concat(path).join(" -> ")); continue; }
        let text;
        try { text = await fetchText(path); } catch (e) {
          report(PV.INCLUDE_FAILED, "error", el, "data-include", "cannot load " + path + ": " + (e && e.message));
          continue;
        }
        const tpl = doc.createElement("template");
        tpl.innerHTML = text;
        const frag = tpl.content;
        // The server adds this script to every HTML file it serves, parts included.
        frag.querySelectorAll("script[src$='/__ui2/preview.js']").forEach((s) => s.remove());
        await expandIncludes(frag, stack.concat(path));
        el.replaceWith(frag);
      }
    }

    const isTemplateRoot = (el) => el.hasAttribute("data-bind-for") || el.hasAttribute("data-bind-if");
    const skipNumbering = (el) => NOT_NODES.has(el.localName) || el.hasAttribute("data-ui2-dev");
    const isTooltipTemplate = (el) => el.localName === "template" && el.getAttribute("data-layer") === "tooltip";
    // The one root element of a tooltip template (null, reported, when it has none or several).
    function tooltipRoot(tpl) {
      const roots = [...tpl.content.children];
      if (roots.length === 1) return roots[0];
      report(E.TOOLTIP, "error", tpl, "data-layer", "a tooltip template holds exactly one element");
      return null;
    }

    function numberDocument(body) {
      let nextTpl = 1;
      const visit = (el, tpl, ctr) => {
        el.setAttribute("data-ui2-node", tpl + ":" + ctr.n++);
        if (el.localName === "svg") return;
        for (const c of el.children) {
          // The root of a tooltip template is a template of its own, numbered where the <template> stands.
          if (isTooltipTemplate(c)) {
            const r = tooltipRoot(c);
            if (r) visit(r, nextTpl++, { n: 0 });
            continue;
          }
          if (skipNumbering(c)) continue;
          if (isTemplateRoot(c)) visit(c, nextTpl++, { n: 0 });
          else visit(c, tpl, ctr);
        }
      };
      const doctr = { n: 0 };
      for (const c of body.children) {
        if (skipNumbering(c)) continue;
        if (isTemplateRoot(c)) visit(c, nextTpl++, { n: 0 });
        else visit(c, 0, doctr);
      }
    }

    // --- data ---

    let sample = { model: {}, state: {}, ui: {}, anchors: {} };
    let l10n = {};
    // The skin state's defaults (`skin.toml [state]`): the converter's server puts the file's text in front of
    // this script (no request of its own). A sample's `state` is written over them.
    let stateDefaults = {};
    let currentEl = null;

    const host = {
      scope: null,
      path(segs) {
        const head = segs[0];
        for (let s = this.scope; s; s = s.parent) {
          if (s.item_name === head) return fromJson(walk(s.item, segs, 1), locText);
          if (s.index_name === head) return segs.length === 1 ? s.index : null;
        }
        const base = head === "state" || head === "ui" || head === "modstate" ? sample[head] : sample.model;
        return fromJson(walk(base, segs, head === "state" || head === "ui" || head === "modstate" ? 1 : 0), locText);
      },
      loc(key, args) { return locText(key, args); },
      fmtInt(n) { return groupInt(n); },
      warn(message) { report(PV.WARN, "warn", currentEl, null, message); },
    };

    // The list keys of the model schema (the converter's server puts them in front of this script).
    const listKeys = typeof window !== "undefined" && window.__ui2keys ? window.__ui2keys : null;

    /** The schema path of a list expression (`top.resources`, `production.options[].requires`), or null. */
    function modelPathOf(expr, scope) {
      if (!expr || expr.t !== "path") return null;
      const segs = expr.segs;
      const head = segs[0];
      for (let s = scope; s; s = s.parent) {
        if (s.item_name === head) return s.item_path ? s.item_path + "[]" + segs.slice(1).map((x) => "." + x).join("") : null;
        if (s.index_name === head) return null;
      }
      return head === "state" || head === "modstate" ? null : segs.join(".");
    }

    function walk(x, segs, from) {
      for (let i = from; i < segs.length; i++) {
        if (!isPlainObject(x) || !Object.prototype.hasOwnProperty.call(x, segs[i])) return null;
        x = x[segs[i]];
      }
      return x;
    }

    function locText(key, args) {
      if (!Object.prototype.hasOwnProperty.call(l10n, key)) return key; // the engine: a missing key is its own text
      const r = locRender(l10n[key], args);
      for (const u of r.unsupported) report(PV.L10N_UNSUPPORTED, "warn", currentEl, "data-l10n", "not previewed: " + u + " in \"" + key + "\"");
      return r.text;
    }

    function ev(expr, scope) { host.scope = scope; return evalExpr(expr, host); }
    function evText(tpl, scope) { host.scope = scope; return renderTemplate(tpl, host); }

    async function loadJson(path) {
      const r = await fetch(new URL(path, rootUrl).href, { cache: "no-store" });
      if (!r.ok) throw new Error("HTTP " + r.status);
      return parseJson(await r.text());
    }
    const okName = (s) => /^[A-Za-z0-9_.-]+$/.test(s);

    async function loadSample(name) {
      if (!okName(name)) { report(PV.LOAD_FAILED, "error", null, null, "bad sample name " + JSON.stringify(name)); return; }
      try {
        const s = await loadJson("sample/" + name + ".json");
        sample = { model: isPlainObject(s) && isPlainObject(s.model) ? s.model : {}, state: mergeState(stateDefaults, s && s.state), ui: (s && s.ui) || {}, anchors: (s && s.anchors) || {}, modstate: (s && s.modstate) || {} };
      } catch (e) {
        report(PV.LOAD_FAILED, "error", null, null, "cannot load sample/" + name + ".json: " + (e && e.message));
      }
    }

    function readStateDefaults() {
      const skin = win.__ui2skin || null;
      if (!skin || typeof skin.toml !== "string") return;
      const t = parseSkinState(skin.toml);
      for (const e of t.errors) report(PV.TOML, "warn", null, "skin.toml:" + e.line, e.message);
      stateDefaults = t.state;
    }

    async function loadL10n(lang) {
      if (!okName(lang)) { report(PV.LOAD_FAILED, "error", null, null, "bad language " + JSON.stringify(lang)); return; }
      try {
        const r = await fetch(new URL("l10n/" + lang + ".toml", rootUrl).href, { cache: "no-store" });
        if (!r.ok) throw new Error("HTTP " + r.status);
        const t = parseToml(await r.text());
        for (const e of t.errors) report(PV.TOML, "warn", null, "l10n/" + lang + ".toml:" + e.line, e.message);
        l10n = t.table;
      } catch (e) {
        report(PV.LOAD_FAILED, "error", null, null, "cannot load l10n/" + lang + ".toml: " + (e && e.message));
      }
    }

    // --- rendering ---

    const parsedCache = new WeakMap();
    function bindingsOf(el) {
      let p = parsedCache.get(el);
      if (!p) {
        p = parseBindings(Array.from(el.attributes, (a) => [a.name, a.value]));
        parsedCache.set(el, p);
      }
      for (const e of p.errors) report(e.code, "error", el, e.attr, e.message);
      return p;
    }

    const extract = () => cfg.mode === "extract";

    function expandChildren(src, out, scope, inst) {
      for (const c of src.childNodes) {
        if (c.nodeType !== 1) { out.appendChild(c.cloneNode(true)); continue; }
        expandElement(c, out, scope, inst);
      }
    }

    function expandElement(el, out, scope, inst) {
      if (isTooltipTemplate(el) && extract()) {
        // Extract mode draws the root of a tooltip template in place of the <template>, so the converter reads its
        // styles; the mark names the template. Its bindings read the model only (no loop variables).
        const r = el.content.children.length === 1 ? el.content.children[0] : null;
        if (!r) return;
        const before = out.lastChild;
        expandElement(r, out, null, "");
        const drawn = out.lastChild;
        if (drawn && drawn !== before && drawn.nodeType === 1) drawn.setAttribute("data-ui2-tooltip", el.id);
        return;
      }
      const p = bindingsOf(el);
      currentEl = el;
      if (p.forSpec) return expandFor(el, p, out, scope, inst);
      if (p.cond) {
        // SIMPLIFIED(SIMP-215): in extract mode every `if` is drawn whatever its condition says, and a `for` over an empty list draws one instance from an empty record, so the converter sees every node once; the data-dependent branches are not all drawn
        if (!extract() && !truthy(ev(p.cond, scope))) return;
      }
      instantiate(el, p, out, scope, inst);
    }

    function expandFor(el, p, out, scope, inst) {
      const spec = p.forSpec;
      const list = ev(spec.list, scope);
      let items = [];
      if (list instanceof Lst && list.items) items = list.items;
      else if (list !== null && !(list instanceof Lst)) report(E.NOT_LIST, "error", el, "data-bind-for", "the list expression is not a list");
      const listPath = modelPathOf(spec.list, scope);
      const keyInfo = listPath !== null && listKeys ? listKeys[listPath] || null : null;
      const cur = { parent: scope, item_name: spec.item, index_name: spec.index, item: null, index: 0, item_path: listPath };
      const withItem = (item, i) => { cur.item = item; cur.index = i; return cur; };
      let plan = planFor(items, spec, {
        item: (expr, item, i) => { currentEl = el; return ev(expr, withItem(item, i)); },
        outer: (expr) => ev(expr, scope),
      }, locText, keyInfo);
      if (extract()) {
        // SIMPLIFIED(SIMP-215): one instance per `for` in extract mode: the first item of the sample, or an empty record when the list has none
        plan = plan.length ? plan.slice(0, 1) : [{ item: {}, key: "0" }];
      }
      const seen = new Set();
      plan.forEach((row, pos) => {
        if (seen.has(row.key)) report(PV.KEY_DUPLICATE, "error", el, "data-bind-key", "two items have the key " + JSON.stringify(row.key));
        seen.add(row.key);
        const path = (inst ? inst + "/" : "") + escKey(row.key);
        const s = { parent: scope, item_name: spec.item, index_name: spec.index, item: row.item, index: pos, item_path: listPath };
        instantiate(el, p, out, s, path);
      });
    }

    function instantiate(el, p, out, scope, inst) {
      if (el.localName === "svg") { const c = el.cloneNode(true); stampInst(c, inst); out.appendChild(c); return; }
      const c = el.cloneNode(false);
      stampInst(c, inst);
      currentEl = el;
      // In extract mode the converter computes the toggled classes as conditions itself and style bindings are
      // runtime values on top of the baked record: the page keeps the markup's own classes and inline styles.
      if (!extract()) {
        for (const t of p.classToggle) c.classList.toggle(t.cls, truthy(ev(t.cond, scope)));
        for (const s of p.styles) c.style.setProperty(s.prop, evText(s.tpl, scope));
      }
      if (p.src && extract()) {
        // A picture the runtime fills from data: the converter needs only the reference, not a placeholder.
        const v = renderValue(ev(p.src, scope));
        if (v !== "") c.setAttribute("data-ui2-asset", v);
      } else if (p.src) {
        const v = renderValue(ev(p.src, scope));
        if (/^[a-z][a-z0-9-]*:/i.test(v)) {
          // A game asset reference: the game's files are not here. Keep the reference, show a placeholder.
          c.setAttribute("data-ui2-asset", v);
          c.setAttribute("src", ASSET_PLACEHOLDER);
        } else if (v !== "") c.setAttribute("src", v);
      }
      // In extract mode a bound `disabled` is a condition the converter reads itself (the `:disabled` variant):
      // the page keeps only the markup's own attribute, or the base values would be read disabled.
      if (p.disabled && !extract()) {
        const off = truthy(ev(p.disabled, scope));
        if (c.localName === "button" || c.localName === "input") c.toggleAttribute("disabled", off);
        else if (off) c.setAttribute("aria-disabled", "true");
      }
      // The placeholder of a text field: a skin text, shown while the field is empty (the runtime does the same).
      // Not in extract mode: the converter reads the key from the markup, and `placeholder` is not in the profile.
      const placeholder = el.getAttribute("data-l10n-placeholder");
      if (placeholder !== null && !extract()) {
        if (!Object.prototype.hasOwnProperty.call(l10n, placeholder)) {
          report(PV.L10N_MISSING, "error", el, "data-l10n-placeholder", "no text for the key " + JSON.stringify(placeholder) + " in l10n/" + cfg.lang + ".toml");
          c.setAttribute("placeholder", placeholder);
        } else c.setAttribute("placeholder", locText(placeholder, []));
      }
      if (el.hasAttribute("data-hotkey-label")) {
        // The name of the key of the nearest data-hotkey (this element or an ancestor), as the runtime shows it.
        const owner = el.closest("[data-hotkey]");
        if (owner) {
          const name = (k, fallback) => Object.prototype.hasOwnProperty.call(l10n, "key." + k) ? locText("key." + k, []) : fallback;
          c.textContent = hotkeyLabel(owner.getAttribute("data-hotkey"), name);
        } else {
          report(E.HOTKEY, "error", el, "data-hotkey-label", "no data-hotkey on the element or an ancestor");
          c.textContent = "";
        }
      } else if (p.value) c.textContent = evText(p.value, scope);
      else if (p.l10n !== null) {
        const args = (p.l10nArgs || []).map((a) => ev(a, scope));
        currentEl = el;
        if (!Object.prototype.hasOwnProperty.call(l10n, p.l10n)) {
          report(PV.L10N_MISSING, "error", el, "data-l10n", "no text for the key " + JSON.stringify(p.l10n) + " in l10n/" + cfg.lang + ".toml");
          c.textContent = p.l10n;
        } else c.textContent = locText(p.l10n, args);
      } else expandChildren(el, c, scope, inst);
      out.appendChild(c);
    }

    function stampInst(c, inst) { if (inst && c.hasAttribute("data-ui2-node")) c.setAttribute("data-ui2-inst", inst); }

    let pristine = null;

    function render() {
      errors.length = staticErrors;
      seenErrors.clear();
      for (const e of errors) seenErrors.add([e.code, e.src, e.node, e.attr, e.message].join("\u0000"));
      const frag = doc.createDocumentFragment();
      host.scope = null;
      for (const n of pristine) {
        if (n.nodeType !== 1) { frag.appendChild(n.cloneNode(true)); continue; }
        expandElement(n, frag, null, "");
      }
      doc.body.replaceChildren(frag);
      return errors;
    }

    async function start() {
      if (ownScript) ownScript.remove();
      try {
        if (!["layout", "extract", "templates"].includes(cfg.mode)) {
          report(PV.MODE, "error", null, null, "unknown mode " + JSON.stringify(cfg.mode) + ", using layout");
          cfg.mode = "layout";
        }
        await expandIncludes(doc.body, []);
        numberDocument(doc.body);
        if (cfg.mode !== "templates") {
          pristine = Array.from(doc.body.childNodes, (n) => n.cloneNode(true));
          readStateDefaults();
          await Promise.all([loadSample(cfg.state), loadL10n(cfg.lang)]);
          staticErrors = errors.length;
          render();
        }
      } catch (e) {
        report("PV-000", "error", null, null, "preview failed: " + (e && e.message));
      }
      staticErrors = Math.min(staticErrors, errors.length);
      resolveReady();
    }

    api.rerender = async function rerender(arg) {
      await ready;
      if (cfg.mode === "templates" || !pristine) return errors;
      if (typeof arg === "string") {
        errors.length = staticErrors;
        cfg.state = arg;
        await loadSample(arg);
        staticErrors = errors.length;
      } else if (isPlainObject(arg)) {
        sample = { model: isPlainObject(arg.model) ? arg.model : {}, state: mergeState(stateDefaults, arg.state), ui: arg.ui || {}, anchors: arg.anchors || {}, modstate: arg.modstate || {} };
      }
      seenErrors.clear();
      errors.splice(0, errors.length, ...errors.slice(0, staticErrors));
      for (const e of errors) seenErrors.add([e.code, e.src, e.node, e.attr, e.message].join("\u0000"));
      return render();
    };

    if (doc.readyState === "loading") doc.addEventListener("DOMContentLoaded", start, { once: true });
    else start();
  }

  core.install = install;
  if (typeof module === "object" && module !== null && typeof module.exports === "object") module.exports = core;
  if (typeof window !== "undefined" && typeof document !== "undefined" && window.document === document) install(window);
})();
