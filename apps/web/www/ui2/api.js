// The `ui` object of UI v2 skin and mod scripts (API 1.0), over a "port" to the main thread.
//
// A classic script (no modules): the worker loader (`worker.js`) takes it with `importScripts`, the mockup shim
// with a <script> tag. It defines `__ui2api` on the global object:
//
//   __ui2api.API                          "1.0.0"
//   __ui2api.createContext(init, port)    -> context; `init` is the `init` message of the supervisor (see the
//                                            ui2-script crate, `proto.rs`); `port.post(obj)` sends a message to
//                                            the main thread, `port.now()` is a clock in ms (optional),
//                                            `port.timers` {set(fn, ms, repeat) -> handle, clear(handle, repeat)}
//                                            replaces `setTimeout`/`setInterval` for `ui.timer` (optional)
//   __ui2api.locate(error)                -> {file, line} of a skin file in the error's stack, or nulls
//   __ui2api.setLineOffset(n)             lines the loader's wrapper adds before line 1 of a file
//
// A context:
//   ctx.ui              the object scripts get
//   ctx.receive(msg)    a `frame` message: apply the model delta, call the subscriptions (once per batch), the
//                       events, then send one `out` that acknowledges the frame
//   ctx.ready()         after the files ran: call the `ready` handlers, send what they and the files wrote
//   ctx.handlers()      the event names given to `ui.on`
//   ctx.rejection(r, w) an unhandled promise rejection of the worker, or (`w` names it) an uncaught error
//
// The model mirror is immutable inside: a change makes new objects along its path, so a subscription sees a
// change by identity. In development mode (`init.dev`) it is frozen, and a script that writes to it throws.
// A handler that throws is switched off alone; one error line goes to the log.
(function (global) {
  "use strict";

  const API = "1.0.0";
  // The list arrays of the mirror and the keys of their elements, in order.
  const KEYS = new WeakMap();
  // A step through a list element (the next key of the op's key chain).
  const ELEM = 0;
  let lineOffset = 0;

  function keysOf(list) {
    let k = KEYS.get(list);
    if (!k) {
      // A list that did not come from a delta (the shim's sample): the index is the key.
      k = list.map((_, i) => String(i));
      KEYS.set(list, k);
    }
    return k;
  }

  function withKeys(list, keys) {
    KEYS.set(list, keys);
    return list;
  }

  function deepFreeze(v) {
    if (v === null || typeof v !== "object" || Object.isFrozen(v)) return;
    Object.freeze(v);
    for (const k of Object.keys(v)) deepFreeze(v[k]);
  }

  function clone(v) {
    return v === undefined ? null : JSON.parse(JSON.stringify(v));
  }

  // ---------------------------------------------------------------- tables

  function rootOf(path, names) {
    let best = null;
    for (const n of names) {
      if (path === n || path.startsWith(n + ".") || path.startsWith(n + "[")) {
        if (best === null || n.length > best.length) best = n;
      }
    }
    return best;
  }

  // `top.res[].n` under root `top` -> ["res", ELEM, "n"].
  function stepsOf(path, root) {
    const steps = [];
    let rest = path.slice(root.length);
    while (rest.length) {
      if (rest.startsWith("[]")) {
        steps.push(ELEM);
        rest = rest.slice(2);
      } else if (rest[0] === ".") {
        const m = /^\.([^.[]+)/.exec(rest);
        steps.push(m[1]);
        rest = rest.slice(m[0].length);
      } else {
        throw new Error("bad path " + path);
      }
    }
    return steps;
  }

  function Tables(schema) {
    this.hash = schema ? schema.hash : null;
    this.roots = new Map();
    this.paths = new Map();
    this.byPath = new Map();
    this.rootNames = [];
    for (const part of schema ? schema.parts : []) {
      const names = part.roots.map((r) => r[1]);
      for (const [id, name, type] of part.roots) this.roots.set(id, { name, type, part });
      part.paths.forEach(([path, type, key, js], i) => {
        const root = rootOf(path, names);
        const e = { id: part.base + i, path, type, key, js: !!js, part, root, steps: stepsOf(path, root) };
        this.paths.set(e.id, e);
        this.byPath.set(path, e);
      });
      this.rootNames.push(...names);
    }
  }

  function decode(t, v, part) {
    if (v === null || v === undefined) return null;
    if (typeof t === "string") return v;
    switch (t[0]) {
      case "enum": {
        const names = part.enums[t[1]];
        return names && typeof v === "number" && v < names.length ? names[v] : v;
      }
      case "opt":
        return decode(t[1], v, part);
      case "struct": {
        const fields = part.types[t[1]];
        const o = {};
        for (let i = 0; i < fields.length; i++) o[fields[i][0]] = decode(fields[i][1], v[i], part);
        return o;
      }
      case "list": {
        const el = ["struct", t[1]];
        const items = new Array(v.length);
        const keys = new Array(v.length);
        for (let i = 0; i < v.length; i++) {
          keys[i] = v[i][0];
          items[i] = decode(el, v[i][1], part);
        }
        return withKeys(items, keys);
      }
      default:
        throw new Error("bad type " + JSON.stringify(t));
    }
  }

  // ---------------------------------------------------------------- outs as bytes
  //
  // Wire 2 (`init.wire`): the outs go as bytes (the main thread reads them without a JSON parser and without
  // an object per island command); every other message stays a JSON text. An out:
  //
  //   u8 3 (out), u8 2 (wire), u8 flags (1: ack), u8 0, f64 n, f64 ack, f64 busy_ms,
  //   u32 counts of state, intents, cmds, settings, log, then each list:
  //   state    str path, str JSON value              intents  str name, str JSON args
  //   cmds     u8 op, str island, str key, then: 0 text (str), 1 game text (str loc, u32 n, str * n),
  //            2 cls (str name, u8 on), 3 style (str prop, str JSON value), 4 show (u8 on)
  //   settings str key, opt str JSON value           log      u8 level (0 info, 1 warn, 2 error), str text,
  //                                                           opt str file, u32 line (or 0xffffffff), opt str key
  //   str = u32 byte length + UTF-8; opt str = a str, or u32 0xffffffff for null

  const WIRE = 2;
  const MSG_OUT = 3;
  const NO = 0xffffffff;
  const utf8 = typeof TextDecoder === "function" ? new TextDecoder() : null;

  // Reads the bytes of a message.
  function Reader(u8) {
    this.u8 = u8;
    this.dv = new DataView(u8.buffer, u8.byteOffset, u8.byteLength);
    this.pos = 0;
  }

  Reader.prototype.byte = function () {
    if (this.pos >= this.u8.length) throw new RangeError("the message ends early");
    return this.u8[this.pos++];
  };
  Reader.prototype.u32 = function () {
    const v = this.dv.getUint32(this.pos, true);
    this.pos += 4;
    return v;
  };
  Reader.prototype.f64 = function () {
    const v = this.dv.getFloat64(this.pos, true);
    this.pos += 8;
    return v;
  };
  // A str written in place (u32 length + UTF-8).
  Reader.prototype.text = function () {
    const n = this.u32();
    if (n === NO) return null;
    const at = this.pos;
    if (at + n > this.u8.length) throw new RangeError("the message ends early");
    this.pos += n;
    return n ? utf8.decode(this.u8.subarray(at, at + n)) : "";
  };

  // Writes outs into one buffer that grows and is kept; each message leaves as a copy of its bytes.
  function Writer() {
    this.buf = new Uint8Array(4096);
    this.dv = new DataView(this.buf.buffer);
    this.pos = 0;
  }

  Writer.prototype.room = function (n) {
    if (this.pos + n <= this.buf.length) return;
    let size = this.buf.length * 2;
    while (size < this.pos + n) size *= 2;
    const b = new Uint8Array(size);
    b.set(this.buf.subarray(0, this.pos));
    this.buf = b;
    this.dv = new DataView(b.buffer);
  };
  Writer.prototype.byte = function (v) {
    this.room(1);
    this.buf[this.pos++] = v;
  };
  Writer.prototype.u32 = function (v) {
    this.room(4);
    this.dv.setUint32(this.pos, v, true);
    this.pos += 4;
  };
  Writer.prototype.f64 = function (v) {
    this.room(8);
    this.dv.setFloat64(this.pos, v, true);
    this.pos += 8;
  };
  // A text as UTF-8, its length first (a lone surrogate becomes U+FFFD).
  Writer.prototype.str = function (s) {
    const len = s.length;
    this.room(4 + 3 * len);
    const b = this.buf;
    const at = this.pos;
    let p = at + 4;
    for (let i = 0; i < len; i++) {
      let c = s.charCodeAt(i);
      if (c < 0x80) {
        b[p++] = c;
      } else if (c < 0x800) {
        b[p++] = 0xc0 | (c >> 6);
        b[p++] = 0x80 | (c & 0x3f);
      } else {
        if (c >= 0xd800 && c <= 0xdfff) {
          const d = i + 1 < len ? s.charCodeAt(i + 1) : 0;
          if (c <= 0xdbff && d >= 0xdc00 && d <= 0xdfff) {
            const cp = 0x10000 + ((c - 0xd800) << 10) + (d - 0xdc00);
            b[p++] = 0xf0 | (cp >> 18);
            b[p++] = 0x80 | ((cp >> 12) & 0x3f);
            b[p++] = 0x80 | ((cp >> 6) & 0x3f);
            b[p++] = 0x80 | (cp & 0x3f);
            i++;
            continue;
          }
          c = 0xfffd;
        }
        b[p++] = 0xe0 | (c >> 12);
        b[p++] = 0x80 | ((c >> 6) & 0x3f);
        b[p++] = 0x80 | (c & 0x3f);
      }
    }
    this.dv.setUint32(at, p - at - 4, true);
    this.pos = p;
  };
  Writer.prototype.opt = function (s) {
    if (s === null || s === undefined) this.u32(NO);
    else this.str(s);
  };
  Writer.prototype.take = function () {
    const out = this.buf.slice(0, this.pos).buffer;
    this.pos = 0;
    return out;
  };

  const LEVELS = { info: 0, warn: 1, error: 2 };
  const CMD_OPS = { text: 0, cls: 2, style: 3, show: 4 };

  // The bytes of an out (the fields of the JSON out message; `ack` null for none).
  function encodeOut(w, n, ack, o, busy) {
    w.pos = 0;
    w.byte(MSG_OUT);
    w.byte(WIRE);
    w.byte(ack === null || ack === undefined ? 0 : 1);
    w.byte(0);
    w.f64(n);
    w.f64(ack === null || ack === undefined ? 0 : ack);
    w.f64(busy);
    w.u32(o.state.length);
    w.u32(o.intents.length);
    w.u32(o.cmds.length);
    w.u32(o.settings.length);
    w.u32(o.log.length);
    for (const s of o.state) {
      w.str(s.p);
      w.str(JSON.stringify(s.v));
    }
    for (const i of o.intents) {
      w.str(i.name);
      w.str(JSON.stringify(i.args));
    }
    for (const c of o.cmds) {
      const v = c.value;
      const loc = c.op === "text" && v !== null && typeof v === "object";
      w.byte(loc ? 1 : CMD_OPS[c.op]);
      w.str(c.island);
      w.str(c.key);
      switch (c.op) {
        case "text":
          if (loc) {
            w.str(v.loc);
            w.u32(v.args.length);
            for (const a of v.args) w.str(a);
          } else {
            w.str(v);
          }
          break;
        case "cls":
          w.str(c.name);
          w.byte(c.on ? 1 : 0);
          break;
        case "style":
          w.str(c.prop);
          w.str(JSON.stringify(v));
          break;
        default:
          w.byte(c.on ? 1 : 0);
      }
    }
    for (const s of o.settings) {
      w.str(s.key);
      w.opt(s.value === null ? null : JSON.stringify(s.value));
    }
    for (const l of o.log) {
      w.byte(LEVELS[l.level] ?? 0);
      w.str(l.text);
      w.opt(l.file);
      w.u32(l.line === null || l.line === undefined ? NO : l.line);
      w.opt(l.key === undefined ? null : l.key);
    }
    return w.take();
  }

  // An out from its bytes, as the JSON out message has it (tests, the bench).
  function decodeOut(data) {
    const u8 = data instanceof Uint8Array ? data : new Uint8Array(data);
    const r = new Reader(u8);
    if (r.byte() !== MSG_OUT || r.byte() !== WIRE) throw new Error("not an out of wire " + WIRE);
    const flags = r.byte();
    r.byte();
    const n = r.f64();
    const ack = r.f64();
    const busy = r.f64();
    const counts = [r.u32(), r.u32(), r.u32(), r.u32(), r.u32()];
    const m = { t: "out", n, ack: flags & 1 ? ack : null, state: [], intents: [], cmds: [], settings: [], log: [], busy_ms: busy };
    for (let i = 0; i < counts[0]; i++) m.state.push({ p: r.text(), v: JSON.parse(r.text()) });
    for (let i = 0; i < counts[1]; i++) m.intents.push({ name: r.text(), args: JSON.parse(r.text()) });
    for (let i = 0; i < counts[2]; i++) {
      const op = r.byte();
      const island = r.text();
      const key = r.text();
      if (op === 0) m.cmds.push({ island, key, op: "text", value: r.text() });
      else if (op === 1) {
        const loc = r.text();
        const args = [];
        for (let j = r.u32(); j > 0; j--) args.push(r.text());
        m.cmds.push({ island, key, op: "text", value: { loc, args } });
      } else if (op === 2) m.cmds.push({ island, key, op: "cls", name: r.text(), on: r.byte() === 1 });
      else if (op === 3) m.cmds.push({ island, key, op: "style", prop: r.text(), value: JSON.parse(r.text()) });
      else if (op === 4) m.cmds.push({ island, key, op: "show", on: r.byte() === 1 });
      else throw new Error("bad island command " + op);
    }
    for (let i = 0; i < counts[3]; i++) {
      const key = r.text();
      const v = r.text();
      m.settings.push({ key, value: v === null ? null : JSON.parse(v) });
    }
    for (let i = 0; i < counts[4]; i++) {
      const level = ["info", "warn", "error"][r.byte()] || "info";
      const text = r.text();
      const file = r.text();
      const line = r.u32();
      const key = r.text();
      const l = { level, text, file, line: line === NO ? null : line };
      if (key !== null) l.key = key;
      m.log.push(l);
    }
    return m;
  }

  // ---------------------------------------------------------------- mirror

  function Mirror(tables, dev) {
    this.t = tables;
    this.dev = dev;
    this.roots = Object.create(null);
    this.revision = 0;
    // The copies the last op of the delta being applied made along its path, and the keys array its list op
    // made (see `own`); empty between deltas.
    this.chain = [];
    this.keysMade = null;
    this.slot = null;
    this.at = [];
    // The last element looked up by key (see `walk`).
    this.lastKeys = null;
    this.lastKey = null;
    this.lastAt = -1;
    this.parsed = new Map();
  }

  // A read path as its root and field names; the ones under a root of the tables are kept.
  Mirror.prototype.parse = function (path) {
    let p = this.parsed.get(path);
    if (p) return p;
    let root = rootOf(path, this.t.rootNames);
    const keep = root !== null;
    if (!keep) root = rootOf(path, Object.keys(this.roots));
    if (root === null) throw new Error("`" + path + "`: unknown model root");
    const rest = path.slice(root.length);
    p = { root, segs: rest ? rest.slice(1).split(".") : [] };
    if (keep && this.parsed.size < 4096) this.parsed.set(path, p);
    return p;
  };

  Mirror.prototype.get = function (path) {
    const p = this.parse(path);
    let node = this.roots[p.root];
    const segs = p.segs;
    if (!segs.length) return node === undefined ? null : node;
    for (let i = 0; i < segs.length; i++) {
      const seg = segs[i];
      if (node === null || node === undefined) return null;
      if (Array.isArray(node)) {
        throw new TypeError("`" + path + "` goes through a list: read the list and look at its elements");
      }
      node = node[seg];
    }
    return node === undefined ? null : node;
  };

  // The position of `key` in the keys `ks` of a list, or -1. A long list keeps an index by key (the keys array of
  // a list is shared by its versions while no element is added, removed or moved).
  const INDEX = new WeakMap();
  const INDEX_MIN = 16;
  const NONE = Object.freeze([]);

  function indexOfKey(ks, key) {
    if (ks.length < INDEX_MIN) return ks.indexOf(key);
    let ix = INDEX.get(ks);
    if (!ix) {
      ix = new Map();
      for (let i = ks.length - 1; i >= 0; i--) ix.set(ks[i], i);
      INDEX.set(ks, ix);
    }
    const at = ix.get(key);
    return at === undefined ? -1 : at;
  }

  // The writes of one delta go to copies of the objects on their paths: what the delta did not touch keeps its
  // identity, what it touched is new, and the model from before the delta is never written. The copies the
  // last op made along its path (`this.chain`, by depth) are changed in place by the next op that goes the same
  // way: the ops of a delta come grouped by element, so a container is copied about once per delta. A copy is
  // reachable only from the copy above it, and the chain is emptied before each delta, so nothing a script has
  // seen is ever changed.
  Mirror.prototype.own = function (node, depth) {
    const chain = this.chain;
    if (depth < chain.length && chain[depth] === node) return node;
    const copy = Array.isArray(node) ? withKeys(node.slice(), keysOf(node)) : Object.assign({}, node);
    chain[depth] = copy;
    return copy;
  };

  // The container that holds the last step of `e` (a root holder, an object or a list), made writable along the
  // path. Returns the holder; `this.slot` is the step into it (a field name or an element index).
  Mirror.prototype.walk = function (e, keys) {
    const steps = e.steps;
    const at = this.at;
    // First the whole path is checked (an op that fails changes nothing), then made writable.
    let node = this.roots[e.root];
    let ki = 0;
    for (let i = 0; i < steps.length; i++) {
      const s = steps[i];
      if (s === ELEM) {
        if (!Array.isArray(node)) throw new Error("model mirror out of step at " + e.path);
        // The ops of a delta come grouped by element: the element of the last lookup is often the one.
        const ks = keysOf(node);
        const key = keys[ki];
        let j;
        if (ks === this.lastKeys && key === this.lastKey) j = this.lastAt;
        else {
          j = indexOfKey(ks, key);
          this.lastKeys = ks;
          this.lastKey = key;
          this.lastAt = j;
        }
        if (j < 0) throw new Error("model mirror has no element " + keys[ki] + " at " + e.path);
        ki++;
        at[i] = j;
        node = node[j];
      } else {
        if (node === null || typeof node !== "object") throw new Error("model mirror out of step at " + e.path);
        at[i] = s;
        node = node[s];
      }
    }
    let holder = this.roots;
    let slot = e.root;
    for (let i = 0; i < steps.length; i++) {
      const next = this.own(holder[slot], i);
      holder[slot] = next;
      holder = next;
      slot = at[i];
    }
    this.slot = slot;
    return holder;
  };

  Mirror.prototype.entry = function (id) {
    const e = this.t.paths.get(id);
    if (!e) throw new Error("unknown field id " + id);
    return e;
  };

  Mirror.prototype.set = function (e, keys, v) {
    const holder = this.walk(e, keys);
    holder[this.slot] = v;
  };

  // The list at `e` made writable, with its keys: [items, keys]. A list that is not there starts empty.
  Mirror.prototype.list = function (e, keys) {
    const holder = this.walk(e, keys);
    const slot = this.slot;
    let items = holder[slot];
    const depth = e.steps.length;
    if (!Array.isArray(items)) {
      items = withKeys([], []);
      this.chain[depth] = items;
    } else {
      items = this.own(items, depth);
    }
    holder[slot] = items;
    let ks = KEYS.get(items);
    if (ks !== this.keysMade) {
      ks = ks.slice();
      this.keysMade = ks;
      KEYS.set(items, ks);
    } else {
      INDEX.delete(ks);
    }
    this.lastKeys = null;
    return items;
  };

  // One op of a UMD1 delta (see ui2-model `umd1.rs`).
  Mirror.prototype.op = function (o) {
    switch (o[0]) {
      case 0: {
        const e = this.entry(o[1]);
        const v = decode(e.type, o[3], e.part);
        this.set(e, o[2], v);
        break;
      }
      case 1: {
        const e = this.entry(o[1]);
        const v = decode(["struct", e.type[1]], o[5], e.part);
        const items = this.list(e, o[2]);
        const keys = KEYS.get(items);
        const at = Math.min(o[3], items.length);
        items.splice(at, 0, v);
        keys.splice(at, 0, o[4]);
        break;
      }
      case 2: {
        const e = this.entry(o[1]);
        const items = this.list(e, o[2]);
        const keys = KEYS.get(items);
        const at = keys.indexOf(o[3]);
        if (at >= 0) {
          items.splice(at, 1);
          keys.splice(at, 1);
        }
        break;
      }
      case 3: {
        const e = this.entry(o[1]);
        const items = this.list(e, o[2]);
        const keys = KEYS.get(items);
        const at = keys.indexOf(o[3]);
        if (at >= 0) {
          const [v] = items.splice(at, 1);
          keys.splice(at, 1);
          const to = Math.min(o[4], items.length);
          items.splice(to, 0, v);
          keys.splice(to, 0, o[3]);
        }
        break;
      }
      case 4: {
        const r = this.t.roots.get(o[1]);
        if (!r) throw new Error("unknown root id " + o[1]);
        this.roots[r.name] = decode(r.type, o[2], r.part);
        break;
      }
      case 5:
        if (this.t.hash && o[1] !== this.t.hash) {
          throw new Error("model schema mismatch: delta " + o[1] + ", tables " + this.t.hash);
        }
        break;
      default:
        throw new Error("unknown UMD1 op " + o[0]);
    }
  };

  // Apply a delta; the ops after a failing one are still tried. Returns the first error or null.
  Mirror.prototype.apply = function (umd1) {
    const a = typeof umd1 === "string" ? JSON.parse(umd1) : umd1;
    let error = null;
    this.revision = a[0];
    this.chain.length = 0;
    this.keysMade = null;
    this.lastKeys = null;
    for (let i = 1; i < a.length; i++) {
      try {
        this.op(a[i]);
      } catch (e) {
        if (!error) error = e;
      }
    }
    this.chain.length = 0;
    this.keysMade = null;
    if (this.dev) for (const k of Object.keys(this.roots)) deepFreeze(this.roots[k]);
    return error;
  };

  // ---------------------------------------------------------------- errors

  // The skin file and line of an error, from the `ui2:/<skin>/<path>` source URL the loader gives each file.
  function locate(e) {
    const stack = e && e.stack ? String(e.stack) : "";
    const m = /ui2:\/[^/\s)]+\/([^\s)]+?):(\d+)/.exec(stack);
    if (!m) return { file: null, line: null };
    return { file: m[1], line: Math.max(1, Number(m[2]) - lineOffset) };
  }

  function messageOf(e) {
    if (e && typeof e === "object" && "message" in e) return String(e.message);
    return String(e);
  }

  // ---------------------------------------------------------------- context

  function createContext(init, port) {
    const now = port.now || (() => Date.now());
    const ctxName = init.ctx || "skin";
    const tables = new Tables(init.schema);
    const mirror = new Mirror(tables, !!init.dev);
    const limits = Object.assign({ slow_ms: 50, log_per_s: 20, timer_min_ms: 50 }, init.limits || {});
    const env = Object.assign({}, init.env || {});
    const ns = init.ns || {};
    const stateRoot = ns.state || "state";
    const intentPrefix = ns.intents || null;
    const reads = init.reads ? init.reads.map((r) => r.replace(/\[\]/g, "")) : null;

    let out = null;
    let outN = 0;
    // Wire 2: the outs go as bytes when the supervisor reads them (`init.wire`) and the port can send them.
    const writer = init.wire >= WIRE && typeof port.postBytes === "function" ? new Writer() : null;
    // State writes not yet seen in the model: full path -> {v, n} (n: the out that carries the write).
    const overlay = new Map();
    // Model subscriptions: {kind: "on" | "list", path, fn | h, last, live, label}.
    const subs = [];
    // Event handlers: name -> [{fn, live, label}].
    const events = new Map();
    let logSecond = -1;
    let logCount = 0;
    // Settings as the actions module keeps them (`[key, JSON text]` in `init`): read here, written through `out`.
    const settings = new Map();
    for (const [k, v] of init.settings || []) {
      try {
        settings.set(k, JSON.parse(v));
      } catch (_) {
        settings.set(k, v);
      }
    }
    // Timers: id -> {fn, repeat, handle, live, label}. The clock functions are taken now: the worker loader may
    // remove globals later. `port.timers` replaces them (tests, the mockup shim).
    const timers = new Map();
    let timerId = 0;
    const clock = port.timers || {
      set: (fn, ms, repeat) => (repeat ? global.setInterval(fn, ms) : global.setTimeout(fn, ms)),
      clear: (handle, repeat) => (repeat ? global.clearInterval(handle) : global.clearTimeout(handle)),
    };

    function fresh() {
      return { state: [], intents: [], cmds: [], settings: [], log: [] };
    }
    out = fresh();

    // A log line. `key` says what an error is about: the supervisor writes a line with a key once per context,
    // also across restarts of the worker (its handlers are switched on again and may fail again).
    function line(level, text, file, ln, key) {
      const l = { level, text, file: file || null, line: ln || null };
      if (key) l.key = key;
      out.log.push(l);
    }

    // `node`: the markup element of the event that called the handler, if it was one.
    function failed(h, e, node) {
      h.live = false;
      const at = locate(e);
      const where = at.file ? " at " + at.file + ":" + at.line : "";
      const msg = messageOf(e);
      const el = node ? " (element " + node + ")" : "";
      line("error", "ui2.js: " + ctxName + " " + h.label + " disabled: " + msg + where + el, at.file, at.line, h.label + "|" + msg);
    }

    // A handler call took `ms`: a slow one is reported once.
    function timed(h, ms) {
      if (ms > limits.slow_ms && !h.slow) {
        h.slow = true;
        line("warn", "ui2.js: " + ctxName + " " + h.label + " slow handler: " + Math.round(ms) + " ms", null, null, h.label + "|slow");
      }
    }

    // Call a handler; an exception switches it off for good, a slow call is reported once.
    function call(h, f, node) {
      if (!h.live) return;
      const t0 = now();
      try {
        f();
      } catch (e) {
        failed(h, e, node);
        return;
      }
      timed(h, now() - t0);
    }

    // `call` of `fn` with `self` as `this` and two or three arguments, without a closure per call (a list
    // subscription calls its handlers once per changed element).
    function call2(h, fn, self, a, b) {
      if (!h.live) return;
      const t0 = now();
      try {
        fn.call(self, a, b);
      } catch (e) {
        failed(h, e, null);
        return;
      }
      timed(h, now() - t0);
    }

    function call3(h, fn, self, a, b, c) {
      if (!h.live) return;
      const t0 = now();
      try {
        fn.call(self, a, b, c);
      } catch (e) {
        failed(h, e, null);
        return;
      }
      timed(h, now() - t0);
    }

    function checkRead(path, what) {
      if (typeof path !== "string") throw new TypeError(what + ": the path must be a string");
      if (!reads) return;
      const p = path.replace(/\[\]/g, "");
      for (const r of reads) if (p === r || p.startsWith(r + ".")) return;
      throw new Error(what + ": `" + path + "` is outside the model this script reads (" + reads.join(", ") + ")");
    }

    function stateTarget(path) {
      if (typeof path !== "string" || !path) throw new TypeError("ui.state: the path must be a non-empty string");
      if (path === "ui" || path.startsWith("ui.")) {
        const e = tables.byPath.get(path);
        if (tables.byPath.size && !(e && e.js)) {
          throw new TypeError("ui.state: `" + path + "` is not a field of `ui` scripts may write");
        }
        return { full: path, wire: path };
      }
      return { full: stateRoot + "." + path, wire: "state." + path };
    }

    function subscribe(kind, path, fn, label) {
      const s = { kind, path, fn, last: mirror.get(path), live: true, label };
      subs.push(s);
      return () => {
        s.live = false;
      };
    }

    function diffList(s, prev, cur) {
      const h = s.fn;
      // The same keys in the same order (only elements changed): element by element, nothing to look up.
      if (Array.isArray(prev) && Array.isArray(cur) && keysOf(prev) === keysOf(cur)) {
        const ks = keysOf(cur);
        for (let i = 0; i < cur.length; i++) {
          const v = cur[i];
          const old = prev[i];
          if (old !== v && h.change) call3(s, h.change, h, ks[i], v, old);
        }
        return;
      }
      // By key (the index of a long list's keys is kept with them, so the next delta finds this one's built).
      const pv = Array.isArray(prev) ? prev : NONE;
      const cv = Array.isArray(cur) ? cur : NONE;
      const pk = pv === NONE ? NONE : keysOf(pv);
      const ck = cv === NONE ? NONE : keysOf(cv);
      for (let i = 0; i < cv.length; i++) {
        const k = ck[i];
        const v = cv[i];
        const j = indexOfKey(pk, k);
        if (j < 0) {
          if (h.add) call2(s, h.add, h, k, v);
        } else if (pv[j] !== v && h.change) {
          call3(s, h.change, h, k, v, pv[j]);
        }
      }
      if (h.remove) {
        for (let i = 0; i < pv.length; i++) if (indexOfKey(ck, pk[i]) < 0) call2(s, h.remove, h, pk[i], pv[i]);
      }
    }

    // After a batch: each subscription whose value changed is called once.
    function notify() {
      const n = subs.length;
      for (let i = 0; i < n; i++) {
        const s = subs[i];
        if (!s.live) continue;
        const cur = mirror.get(s.path);
        if (cur === s.last) continue;
        const prev = s.last;
        s.last = cur;
        if (s.kind === "on") call2(s, s.fn, s, cur, prev);
        else diffList(s, prev, cur);
      }
      for (let i = subs.length - 1; i >= 0; i--) if (!subs[i].live) subs.splice(i, 1);
    }

    function fire(name, arg, info) {
      const list = events.get(name);
      if (!list) return;
      for (const h of list.slice()) call(h, () => h.fn(arg, info), info && info.node);
    }

    function empty() {
      return !out.state.length && !out.intents.length && !out.cmds.length && !out.settings.length && !out.log.length;
    }

    // Send what was collected. A frame is always acknowledged; otherwise only a non-empty out is sent.
    function flush(ack, t0) {
      if (ack === null && empty()) return;
      outN += 1;
      const busy = now() - t0;
      const o = out;
      out = fresh();
      if (writer) port.postBytes(encodeOut(writer, outN, ack, o, busy));
      else port.post(Object.assign({ t: "out", n: outN, ack }, o, { busy_ms: busy }));
    }

    const ui = {
      api: API,
      env,
      model: {
        get(path) {
          checkRead(path, "ui.model.get");
          return mirror.get(path);
        },
        on(path, fn) {
          checkRead(path, "ui.model.on");
          return subscribe("on", path, fn, "model.on(" + path + ")");
        },
        onList(path, handlers) {
          checkRead(path, "ui.model.onList");
          if (path.includes("[]")) {
            throw new TypeError("ui.model.onList: `" + path + "` is a list inside a list; subscribe to the outer list");
          }
          return subscribe("list", path, handlers || {}, "model.onList(" + path + ")");
        },
      },
      state: {
        get(path) {
          const t = stateTarget(path);
          const o = overlay.get(t.full);
          return o ? o.v : mirror.get(t.full);
        },
        set(path, value) {
          const t = stateTarget(path);
          const v = clone(value);
          out.state.push({ p: t.wire, v });
          overlay.set(t.full, { v, n: outN + 1 });
        },
        merge(path, obj) {
          for (const k of Object.keys(obj || {})) ui.state.set(path ? path + "." + k : k, obj[k]);
        },
        on(path, fn) {
          const t = stateTarget(path);
          return subscribe("on", t.full, fn, "state.on(" + path + ")");
        },
      },
      intent(name, args) {
        if (typeof name !== "string" || !name) {
          line("error", "ui2.js: " + ctxName + " ui.intent needs a name");
          return;
        }
        if (intentPrefix && !name.startsWith(intentPrefix)) {
          line("error", "ui2.js: " + ctxName + " intent `" + name + "` is not one of `" + intentPrefix + "*`");
          return;
        }
        out.intents.push({ name, args: args ? clone(args) : {} });
      },
      on(name, fn) {
        if (typeof name !== "string" || typeof fn !== "function") {
          throw new TypeError("ui.on(name, fn): a name and a function");
        }
        const h = { fn, live: true, label: "on(" + name + ")" };
        if (!events.has(name)) events.set(name, []);
        events.get(name).push(h);
        return () => {
          h.live = false;
        };
      },
      island(id) {
        if (typeof id !== "string" || !id) throw new TypeError("ui.island(id): the id of a data-ui2-island");
        const checkKey = (key) => {
          if (typeof key !== "string" || !key) throw new TypeError("ui.island(" + id + "): the key of a data-ui2-key");
        };
        const isl = {
          // `s`: a string, or {loc, args} for a game text with its arguments.
          text(key, s) {
            const value =
              s !== null && typeof s === "object" && "loc" in s
                ? { loc: String(s.loc), args: (s.args || []).map((a) => String(a)) }
                : s === null || s === undefined
                  ? ""
                  : String(s);
            checkKey(key);
            out.cmds.push({ island: id, key, op: "text", value });
            return isl;
          },
          cls(key, name, on) {
            if (typeof name !== "string" || !name) throw new TypeError("ui.island(" + id + ").cls: a class name");
            checkKey(key);
            out.cmds.push({ island: id, key, op: "cls", name, on: on === undefined ? true : !!on });
            return isl;
          },
          style(key, prop, value) {
            if (typeof prop !== "string" || !prop) throw new TypeError("ui.island(" + id + ").style: a property");
            const v = value === undefined ? null : clone(value);
            checkKey(key);
            out.cmds.push({ island: id, key, op: "style", prop, value: v });
            return isl;
          },
          show(key, on) {
            checkKey(key);
            out.cmds.push({ island: id, key, op: "show", on: on === undefined ? true : !!on });
            return isl;
          },
        };
        return isl;
      },
      settings: {
        // The value as last set (a copy), or null.
        get(key) {
          return settings.has(key) ? clone(settings.get(key)) : null;
        },
        // Any JSON value; null removes the key. The actions module keeps it and applies its limits.
        set(key, value) {
          if (typeof key !== "string" || !key) throw new TypeError("ui.settings.set(key, value): a key");
          const v = clone(value);
          if (v === null) settings.delete(key);
          else settings.set(key, v);
          out.settings.push({ key, value: v });
        },
        remove(key) {
          ui.settings.set(key, null);
        },
      },
      timer: {
        // Call `fn` once after `ms` (at least the limit of `init`); returns an id for `cancel`.
        after(ms, fn) {
          return startTimer(ms, fn, false);
        },
        // Call `fn` every `ms` until cancelled.
        every(ms, fn) {
          return startTimer(ms, fn, true);
        },
        cancel(id) {
          const t = timers.get(id);
          if (!t) return;
          t.live = false;
          clock.clear(t.handle, t.repeat);
          timers.delete(id);
        },
      },
      log: {
        info(...args) {
          scriptLog("info", args);
        },
        warn(...args) {
          scriptLog("warn", args);
        },
      },
    };

    // A timer's call is a handler call (an exception switches the timer off, a slow call is reported); what it
    // wrote goes out at once, unacknowledged.
    function startTimer(ms, fn, repeat) {
      if (typeof fn !== "function") throw new TypeError("ui.timer: a function");
      const wait = Math.max(Number(ms) || 0, limits.timer_min_ms);
      const id = ++timerId;
      const t = { fn, repeat, live: true, label: (repeat ? "timer.every(" : "timer.after(") + wait + ")" };
      t.handle = clock.set(
        () => {
          if (!t.live) return;
          const t0 = now();
          call(t, () => t.fn());
          if (!t.repeat || !t.live) ui.timer.cancel(id);
          flush(null, t0);
        },
        wait,
        repeat,
      );
      timers.set(id, t);
      return id;
    }

    function scriptLog(level, args) {
      const sec = Math.floor(now() / 1000);
      if (sec !== logSecond) {
        logSecond = sec;
        logCount = 0;
      }
      logCount += 1;
      if (logCount > limits.log_per_s) return;
      const at = locate(new Error());
      line(level, args.map((a) => (typeof a === "string" ? a : JSON.stringify(a))).join(" "), at.file, at.line);
    }

    // The snapshot of `init`, or the decoded roots the shim passes.
    if (init.model) {
      const e = mirror.apply(init.model);
      if (e) line("error", "ui2.js: " + ctxName + " model snapshot: " + messageOf(e), null, null, "model|" + messageOf(e));
    }
    if (init.roots) for (const k of Object.keys(init.roots)) mirror.roots[k] = init.roots[k];
    if (init.dev) for (const k of Object.keys(mirror.roots)) deepFreeze(mirror.roots[k]);

    // The path of a frame is run on a copy of the snapshot before the first frame comes (the engine compiles
    // code once it ran a few times; a first frame through cold code takes several times as long). Nothing of it
    // is seen: its own mirror, a list handler that does nothing, an out that is not sent.
    function warmUp() {
      if (!writer || !Array.isArray(init.model)) return;
      const m = new Mirror(tables, false);
      m.apply(init.model);
      // A set of each scalar field of the first elements of each list one level down, to the value it has.
      const ops = [];
      for (const e of tables.paths.values()) {
        if (typeof e.type !== "string" || e.steps.length < 2 || e.steps[e.steps.length - 1] === ELEM) continue;
        const at = e.steps.indexOf(ELEM);
        if (at < 0 || e.steps.indexOf(ELEM, at + 1) >= 0) continue;
        let node = m.roots[e.root];
        for (let i = 0; i < at && node !== null && node !== undefined; i++) node = node[e.steps[i]];
        if (!Array.isArray(node)) continue;
        const ks = keysOf(node);
        for (let i = 0; i < node.length && i < 64; i++) {
          let v = node[i];
          for (let j = at + 1; j < e.steps.length && v !== null && v !== undefined; j++) v = v[e.steps[j]];
          if (v !== undefined) ops.push([0, e.id, [ks[i]], v]);
        }
      }
      if (!ops.length) return;
      // The lists outside lists, before and after each delta, through a list handler that does nothing.
      const lists = [];
      for (const e of tables.paths.values()) if (Array.isArray(e.type) && e.type[0] === "list" && e.steps.indexOf(ELEM) < 0) lists.push(e);
      const listAt = (e) => {
        let node = m.roots[e.root];
        for (let i = 0; i < e.steps.length && node !== null && node !== undefined; i++) node = node[e.steps[i]];
        return node;
      };
      const sub = { kind: "list", fn: { add() {}, change() {}, remove() {} }, live: true, slow: true, label: "" };
      const resets = [1].concat(init.model.slice(1).filter((o) => o[0] === 4));
      const sets = [2].concat(ops);
      const o = fresh();
      for (let round = 0; round < 8; round++) {
        const before = lists.map(listAt);
        m.apply(round === 0 ? resets : sets);
        lists.forEach((e, i) => diffList(sub, before[i], listAt(e)));
        o.cmds.push({ island: "warm", key: "k", op: "text", value: "" }, { island: "warm", key: "k", op: "show", on: true });
        encodeOut(writer, 0, 0, o, 0);
      }
    }
    warmUp();

    return {
      ui,
      receive(msg) {
        const t0 = now();
        if (!msg || msg.t !== "frame") return;
        if (typeof msg.applied === "number") {
          for (const [p, o] of overlay) if (o.n <= msg.applied) overlay.delete(p);
        }
        if (msg.model) {
          const e = mirror.apply(msg.model);
          if (e) line("error", "ui2.js: " + ctxName + " model delta: " + messageOf(e), null, null, "model|" + messageOf(e));
          notify();
        }
        if (msg.env) {
          Object.assign(env, msg.env);
          fire("env", env);
        }
        for (const ev of msg.events || []) fire(ev.name, ev.arg, { node: ev.node });
        flush(msg.seq === undefined ? null : msg.seq, t0);
      },
      ready() {
        const t0 = now();
        fire("ready");
        flush(null, t0);
      },
      handlers() {
        return Array.from(events.keys());
      },
      // An unhandled promise rejection (`what` empty) or an uncaught error of the worker (`what` names it).
      rejection(reason, what) {
        const at = locate(reason);
        const where = at.file ? " at " + at.file + ":" + at.line : "";
        line("error", "ui2.js: " + ctxName + " " + (what || "unhandled rejection") + ": " + messageOf(reason) + where, at.file, at.line, (what || "rejection") + "|" + messageOf(reason));
        flush(null, now());
      },
    };
  }

  global.__ui2api = {
    API,
    WIRE,
    createContext,
    locate,
    decodeOut,
    setLineOffset(n) {
      lineOffset = n;
    },
  };
})(globalThis);
