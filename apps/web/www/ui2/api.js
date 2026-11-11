// The `ui` object of UI v2 skin and mod scripts (API 1.0), over a "port" to the main thread.
//
// A classic script (no modules): the worker loader (`worker.js`) takes it with `importScripts`, the mockup shim
// with a <script> tag. It defines `__ui2api` on the global object:
//
//   __ui2api.API                          "1.0.0"
//   __ui2api.createContext(init, port)    -> context; `init` is the `init` message of the supervisor (see the
//                                            ui2-script crate, `proto.rs`); `port.post(obj)` sends a message to
//                                            the main thread, `port.now()` is a clock in ms (optional)
//   __ui2api.locate(error)                -> {file, line} of a skin file in the error's stack, or nulls
//   __ui2api.setLineOffset(n)             lines the loader's wrapper adds before line 1 of a file
//
// A context:
//   ctx.ui              the object scripts get
//   ctx.receive(msg)    a `frame` message: apply the model delta, call the subscriptions (once per batch), the
//                       events, then send one `out` that acknowledges the frame
//   ctx.ready()         after the files ran: call the `ready` handlers, send what they and the files wrote
//   ctx.handlers()      the event names given to `ui.on`
//   ctx.rejection(r)    an unhandled promise rejection of the worker
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

  // ---------------------------------------------------------------- mirror

  function Mirror(tables, dev) {
    this.t = tables;
    this.dev = dev;
    this.roots = Object.create(null);
    this.revision = 0;
  }

  Mirror.prototype.rootFor = function (path) {
    const r = rootOf(path, this.t.rootNames);
    return r !== null ? r : rootOf(path, Object.keys(this.roots));
  };

  Mirror.prototype.get = function (path) {
    const root = this.rootFor(path);
    if (root === null) throw new Error("`" + path + "`: unknown model root");
    let node = this.roots[root];
    const rest = path.slice(root.length);
    if (!rest) return node === undefined ? null : node;
    for (const seg of rest.slice(1).split(".")) {
      if (node === null || node === undefined) return null;
      if (Array.isArray(node)) {
        throw new TypeError("`" + path + "` goes through a list: read the list and look at its elements");
      }
      node = node[seg];
    }
    return node === undefined ? null : node;
  };

  function updateIn(node, steps, i, keys, ki, fn, path) {
    if (i === steps.length) return fn(node);
    const s = steps[i];
    if (s === ELEM) {
      if (!Array.isArray(node)) throw new Error("model mirror out of step at " + path);
      const ks = keysOf(node);
      const at = ks.indexOf(keys[ki]);
      if (at < 0) throw new Error("model mirror has no element " + keys[ki] + " at " + path);
      const copy = withKeys(node.slice(), ks);
      copy[at] = updateIn(node[at], steps, i + 1, keys, ki + 1, fn, path);
      return copy;
    }
    if (node === null || typeof node !== "object") throw new Error("model mirror out of step at " + path);
    const copy = Object.assign({}, node);
    copy[s] = updateIn(node[s], steps, i + 1, keys, ki, fn, path);
    return copy;
  }

  Mirror.prototype.entry = function (id) {
    const e = this.t.paths.get(id);
    if (!e) throw new Error("unknown field id " + id);
    return e;
  };

  Mirror.prototype.update = function (e, keys, fn) {
    this.roots[e.root] = updateIn(this.roots[e.root], e.steps, 0, keys, 0, fn, e.path);
  };

  Mirror.prototype.listOp = function (e, keys, fn) {
    this.update(e, keys, (list) => {
      const old = Array.isArray(list) ? list : [];
      return fn(old.slice(), keysOf(old).slice());
    });
  };

  // One op of a UMD1 delta (see ui2-model `umd1.rs`).
  Mirror.prototype.op = function (o) {
    switch (o[0]) {
      case 0: {
        const e = this.entry(o[1]);
        const v = decode(e.type, o[3], e.part);
        this.update(e, o[2], () => v);
        break;
      }
      case 1: {
        const e = this.entry(o[1]);
        const v = decode(["struct", e.type[1]], o[5], e.part);
        this.listOp(e, o[2], (items, keys) => {
          const at = Math.min(o[3], items.length);
          items.splice(at, 0, v);
          keys.splice(at, 0, o[4]);
          return withKeys(items, keys);
        });
        break;
      }
      case 2: {
        const e = this.entry(o[1]);
        this.listOp(e, o[2], (items, keys) => {
          const at = keys.indexOf(o[3]);
          if (at >= 0) {
            items.splice(at, 1);
            keys.splice(at, 1);
          }
          return withKeys(items, keys);
        });
        break;
      }
      case 3: {
        const e = this.entry(o[1]);
        this.listOp(e, o[2], (items, keys) => {
          const at = keys.indexOf(o[3]);
          if (at >= 0) {
            const [v] = items.splice(at, 1);
            keys.splice(at, 1);
            const to = Math.min(o[4], items.length);
            items.splice(to, 0, v);
            keys.splice(to, 0, o[3]);
          }
          return withKeys(items, keys);
        });
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
    for (let i = 1; i < a.length; i++) {
      try {
        this.op(a[i]);
      } catch (e) {
        if (!error) error = e;
      }
    }
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
    // State writes not yet seen in the model: full path -> {v, n} (n: the out that carries the write).
    const overlay = new Map();
    // Model subscriptions: {kind: "on" | "list", path, fn | h, last, live, label}.
    const subs = [];
    // Event handlers: name -> [{fn, live, label}].
    const events = new Map();
    let logSecond = -1;
    let logCount = 0;

    function fresh() {
      return { state: [], intents: [], cmds: [], settings: [], log: [] };
    }
    out = fresh();

    function line(level, text, file, ln) {
      out.log.push({ level, text, file: file || null, line: ln || null });
    }

    function failed(h, e) {
      h.live = false;
      const at = locate(e);
      const where = at.file ? " at " + at.file + ":" + at.line : "";
      line("error", "ui2.js: " + ctxName + " " + h.label + " disabled: " + messageOf(e) + where, at.file, at.line);
    }

    // Call a handler; an exception switches it off for good, a slow call is reported once.
    function call(h, f) {
      if (!h.live) return;
      const t0 = now();
      try {
        f();
      } catch (e) {
        failed(h, e);
        return;
      }
      const ms = now() - t0;
      if (ms > limits.slow_ms && !h.slow) {
        h.slow = true;
        line("warn", "ui2.js: " + ctxName + " " + h.label + " slow handler: " + Math.round(ms) + " ms");
      }
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
      const before = new Map();
      if (Array.isArray(prev)) {
        const ks = keysOf(prev);
        prev.forEach((v, i) => before.set(ks[i], v));
      }
      const after = new Set();
      if (Array.isArray(cur)) {
        const ks = keysOf(cur);
        cur.forEach((v, i) => {
          const k = ks[i];
          after.add(k);
          if (!before.has(k)) {
            if (h.add) call(s, () => h.add(k, v));
          } else if (before.get(k) !== v && h.change) {
            call(s, () => h.change(k, v, before.get(k)));
          }
        });
      }
      if (h.remove) {
        for (const [k, v] of before) if (!after.has(k)) call(s, () => h.remove(k, v));
      }
    }

    // After a batch: each subscription whose value changed is called once.
    function notify() {
      for (const s of subs.slice()) {
        if (!s.live) continue;
        const cur = mirror.get(s.path);
        if (cur === s.last) continue;
        const prev = s.last;
        s.last = cur;
        if (s.kind === "on") call(s, () => s.fn(cur, prev));
        else diffList(s, prev, cur);
      }
      for (let i = subs.length - 1; i >= 0; i--) if (!subs[i].live) subs.splice(i, 1);
    }

    function fire(name, arg, info) {
      const list = events.get(name);
      if (!list) return;
      for (const h of list.slice()) call(h, () => h.fn(arg, info));
    }

    function empty() {
      return !out.state.length && !out.intents.length && !out.cmds.length && !out.settings.length && !out.log.length;
    }

    // Send what was collected. A frame is always acknowledged; otherwise only a non-empty out is sent.
    function flush(ack, t0) {
      if (ack === null && empty()) return;
      outN += 1;
      const msg = Object.assign({ t: "out", n: outN, ack }, out, { busy_ms: now() - t0 });
      out = fresh();
      port.post(msg);
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
      log: {
        info(...args) {
          scriptLog("info", args);
        },
        warn(...args) {
          scriptLog("warn", args);
        },
      },
    };

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
      if (e) line("error", "ui2.js: " + ctxName + " model snapshot: " + messageOf(e));
    }
    if (init.roots) for (const k of Object.keys(init.roots)) mirror.roots[k] = init.roots[k];
    if (init.dev) for (const k of Object.keys(mirror.roots)) deepFreeze(mirror.roots[k]);

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
          if (e) line("error", "ui2.js: " + ctxName + " model delta: " + messageOf(e));
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
      rejection(reason) {
        const at = locate(reason);
        const where = at.file ? " at " + at.file + ":" + at.line : "";
        line("error", "ui2.js: " + ctxName + " unhandled rejection: " + messageOf(reason) + where, at.file, at.line);
        flush(null, now());
      },
    };
  }

  global.__ui2api = {
    API,
    createContext,
    locate,
    setLineOffset(n) {
      lineOffset = n;
    },
  };
})(globalThis);
