// The mockup shim of UI v2 skins: a skin's source page, opened in a plain browser (also from `file://`), runs its
// scripts with the same `ui` object the game gives them in a worker.
//
// A classic script (no modules: `file://` does not load them). The page loads, in this order:
//
//   <script src="<repo>/apps/web/www/ui2/api.js" data-ui2-dev></script>
//   <script src="../_shim/ui2-shim.js" data-ui2-dev></script>
//   ... the skin's own scripts, as classic <script> tags (they see `ui` as a global) ...
//
// What the shim does:
// 1. makes `ui` with `api.js` over a port inside the page. The model, `state` and `ui` are a sample of the skin
//    (`{model, state, ui}`, as `sample/<state>.json`): `window.__ui2sample` set before the shim, else a
//    `<script type="application/json" data-ui2-sample>` element, else `sample/<state>.json` fetched relative to the
//    page (`?state=<name>`, `hud` by default; served pages only: `file://` cannot fetch);
// 2. sends the scripts the markup's events: `data-event` (a click, with `data-event-arg`) and `call name(args)` of
//    `data-on-<event>`, also from `data-hotkey`; `set` and `toggle` of `data-on-<event>` write the state as the
//    game does;
// 3. applies to the page the bindings to `state.*` and `ui.*` only: `data-bind-class-toggle`, `data-bind-if` and
//    `data-bind-value`;
// 4. carries out island commands on the page; intents go to the console and to `window.__ui2shim.intents`.
//
// `?ui2shim=off`: `ui` exists (the scripts load without errors) but the shim leaves the page and its events alone.
//
// SIMPLIFIED(SIMP-260): the shim re-renders only bindings to `state.*` and `ui.*`; lists (`data-bind-for`) and
// bindings to the rest of the model keep the mockup's static markup, so sorting and filtering are not visible here.
(function (global) {
  "use strict";

  const doc = global.document;
  const api = global.__ui2api;
  const params = new URLSearchParams(global.location ? global.location.search : "");
  const off = params.get("ui2shim") === "off";
  const sampleName = params.get("state") || "hud";

  // The roots of the model (schema 1.0) plus the skin's state: a delta can replace any of them.
  const ROOTS = [
    "meta", "game", "top", "research", "players", "turn", "unit", "map", "notes", "chat", "production", "ui", "state",
  ];
  // The DOM events markup may handle; the ones that do not bubble are handled on their own element only.
  const EVENTS = ["click", "dblclick", "contextmenu", "pointerdown", "pointerup", "input", "change", "submit", "keydown", "keyup"];
  const OWN_EVENTS = ["pointerenter", "pointerleave", "focus", "blur"];
  // CSS properties whose numbers have no unit.
  const UNITLESS = new Set(["opacity", "z-index", "flex-grow", "flex-shrink", "order", "font-weight", "line-height"]);

  const shim = {
    intents: [],
    settings: {},
    log: [],
    frames: 0,
    ctx: null,
    ui: null,
    // The model as the shim holds it (roots by name).
    roots: Object.create(null),
    // Resolves when the scripts got `ready`.
    ready: null,
  };
  global.__ui2shim = shim;

  if (!api) {
    console.error("ui2 shim: load apps/web/www/ui2/api.js before ui2-shim.js");
    return;
  }

  // ---------------------------------------------------------------- the model

  function setRoots(sample) {
    const s = sample || {};
    const model = s.model || {};
    for (const k of Object.keys(model)) shim.roots[k] = model[k];
    shim.roots.state = s.state || shim.roots.state || {};
    shim.roots.ui = Object.assign({}, model.ui || {}, s.ui || {});
  }

  function inlineSample() {
    if (global.__ui2sample) return global.__ui2sample;
    const el = doc && doc.querySelector("script[type='application/json'][data-ui2-sample]");
    if (!el) return null;
    try {
      return JSON.parse(el.textContent);
    } catch (e) {
      console.warn("ui2 shim: the data-ui2-sample element is not JSON: " + e.message);
      return null;
    }
  }

  // A new root with `value` at the dotted `path` below it; the objects along the path are copies.
  function setIn(node, steps, value) {
    if (!steps.length) return value;
    const copy = node && typeof node === "object" && !Array.isArray(node) ? Object.assign({}, node) : {};
    copy[steps[0]] = setIn(copy[steps[0]], steps.slice(1), value);
    return copy;
  }

  function get(path) {
    const steps = path.split(".");
    let node = shim.roots[steps[0]];
    for (const s of steps.slice(1)) {
      if (node === null || node === undefined) return null;
      node = node[s];
    }
    return node === undefined ? null : node;
  }

  // ---------------------------------------------------------------- expressions

  // A small reader of the profile's expressions: literals, paths, `! - + == != < <= > >= && ||` and parentheses.
  // A call or a loop variable is not known here: `Unknown` is thrown and the binding is left as written.
  class Unknown extends Error {}

  function tokens(src) {
    const out = [];
    const re = /\s*(?:(\d+(?:\.\d+)?)|('(?:[^'\\]|\\.)*'|"(?:[^"\\]|\\.)*")|([A-Za-z_]\w*(?:\.[A-Za-z_]\w*)*)|(==|!=|<=|>=|&&|\|\||[!<>()+\-,:]))/y;
    let i = 0;
    while (i < src.length) {
      if (/^\s*$/.test(src.slice(i))) break;
      re.lastIndex = i;
      const m = re.exec(src);
      if (!m) throw new Unknown("cannot read `" + src + "`");
      if (m[1] !== undefined) out.push({ k: "num", v: Number(m[1]) });
      else if (m[2] !== undefined) out.push({ k: "str", v: m[2].slice(1, -1).replace(/\\(.)/g, "$1") });
      else if (m[3] !== undefined) out.push({ k: "id", v: m[3] });
      else out.push({ k: "op", v: m[4] });
      i = re.lastIndex;
    }
    return out;
  }

  // Parses an expression into a function of nothing (it reads the shim's model) and the paths it reads.
  function compile(src) {
    const t = tokens(src);
    let p = 0;
    const paths = [];
    const peek = (v) => t[p] && t[p].k === "op" && t[p].v === v;
    function primary() {
      const tok = t[p++];
      if (!tok) throw new Unknown("`" + src + "` ends early");
      if (tok.k === "num" || tok.k === "str") return () => tok.v;
      if (tok.k === "op" && tok.v === "(") {
        const e = or();
        if (!peek(")")) throw new Unknown("`" + src + "`: a ) is missing");
        p++;
        return e;
      }
      if (tok.k === "id") {
        if (tok.v === "true") return () => true;
        if (tok.v === "false") return () => false;
        if (tok.v === "null") return () => null;
        if (peek("(")) throw new Unknown("`" + src + "`: functions are not evaluated by the shim");
        const root = tok.v.split(".")[0];
        if (!ROOTS.includes(root)) throw new Unknown("`" + tok.v + "` is a loop variable");
        paths.push(tok.v);
        return () => get(tok.v);
      }
      throw new Unknown("`" + src + "`: unexpected " + tok.v);
    }
    function unary() {
      if (peek("!")) {
        p++;
        const e = unary();
        return () => !e();
      }
      if (peek("-")) {
        p++;
        const e = unary();
        return () => -e();
      }
      return primary();
    }
    function sum() {
      let e = unary();
      while (peek("+") || peek("-")) {
        const op = t[p++].v;
        const a = e;
        const b = unary();
        e = op === "+" ? () => a() + b() : () => a() - b();
      }
      return e;
    }
    function cmp() {
      const a = sum();
      for (const op of ["==", "!=", "<=", ">=", "<", ">"]) {
        if (peek(op)) {
          p++;
          const b = sum();
          return {
            "==": () => a() === b(),
            "!=": () => a() !== b(),
            "<=": () => a() <= b(),
            ">=": () => a() >= b(),
            "<": () => a() < b(),
            ">": () => a() > b(),
          }[op];
        }
      }
      return a;
    }
    function and() {
      let e = cmp();
      while (peek("&&")) {
        p++;
        const a = e;
        const b = cmp();
        e = () => a() && b();
      }
      return e;
    }
    function or() {
      let e = and();
      while (peek("||")) {
        p++;
        const a = e;
        const b = and();
        e = () => a() || b();
      }
      return e;
    }
    const fn = or();
    if (p !== t.length) throw new Unknown("`" + src + "`: more after the expression");
    return { fn, paths };
  }

  // Whether the shim may show a binding: every path it reads is in `state` or `ui` (SIMP-260).
  const local = (paths) => paths.every((x) => x === "state" || x === "ui" || x.startsWith("state.") || x.startsWith("ui."));

  function inner(src) {
    const m = /^\s*\{\{([\s\S]*)\}\}\s*$/.exec(src);
    return m ? m[1] : src;
  }

  // ---------------------------------------------------------------- bindings

  const bound = new Map();

  function bindingsOf(el) {
    let b = bound.get(el);
    if (b) return b;
    b = { ifs: null, cls: [], value: null };
    const tryCompile = (src) => {
      try {
        const c = compile(src);
        return local(c.paths) ? c : null;
      } catch (e) {
        if (e instanceof Unknown) return null;
        throw e;
      }
    };
    const ifs = el.getAttribute("data-bind-if");
    if (ifs !== null) b.ifs = tryCompile(inner(ifs));
    const ct = el.getAttribute("data-bind-class-toggle");
    if (ct !== null) {
      for (const part of ct.split(";")) {
        const at = part.indexOf(":");
        if (at < 0) continue;
        const c = tryCompile(inner(part.slice(at + 1)));
        if (c) b.cls.push({ name: part.slice(0, at).trim(), c });
      }
    }
    const val = el.getAttribute("data-bind-value");
    if (val !== null) {
      const pieces = [];
      let ok = true;
      const re = /\{\{([\s\S]*?)\}\}/g;
      let last = 0;
      let m;
      while ((m = re.exec(val))) {
        pieces.push(val.slice(last, m.index));
        const c = tryCompile(m[1]);
        if (!c) ok = false;
        pieces.push(c);
        last = re.lastIndex;
      }
      pieces.push(val.slice(last));
      if (ok) b.value = pieces;
    }
    bound.set(el, b);
    return b;
  }

  function render() {
    if (off || !doc) return;
    for (const el of doc.querySelectorAll("[data-bind-if],[data-bind-class-toggle],[data-bind-value]")) {
      const b = bindingsOf(el);
      if (b.ifs) el.style.display = b.ifs.fn() ? "" : "none";
      for (const c of b.cls) el.classList.toggle(c.name, !!c.c.fn());
      if (b.value) {
        const text = b.value.map((x) => (typeof x === "string" ? x : String(x.fn() ?? ""))).join("");
        if (el.textContent !== text) el.textContent = text;
      }
    }
  }

  // ---------------------------------------------------------------- the port

  let seq = 0;
  let revision = 0;
  const inbox = [];
  let scheduled = false;

  function rootId(name) {
    return ROOTS.indexOf(name);
  }

  // A frame with the roots that changed (whole) and the markup's events.
  function sendFrame(changed, events, applied) {
    const msg = { t: "frame", seq: ++seq, events: events || [] };
    if (typeof applied === "number") msg.applied = applied;
    if (changed && changed.size) {
      revision += 1;
      msg.model = [revision];
      for (const r of changed) msg.model.push([4, rootId(r), shim.roots[r] === undefined ? null : shim.roots[r]]);
    }
    shim.frames += 1;
    shim.ctx.receive(msg);
  }

  // Writes of the scripts or the markup to the state: `state.<path>` or a field of `ui`.
  function writeState(path, value, changed) {
    const steps = path.split(".");
    if (!ROOTS.includes(steps[0])) return;
    shim.roots[steps[0]] = setIn(shim.roots[steps[0]], steps.slice(1), value);
    changed.add(steps[0]);
  }

  function island(cmd) {
    if (!doc) return;
    const host = doc.querySelector("[data-ui2-island='" + cmd.island + "']");
    if (!host) return;
    const el = host.getAttribute("data-ui2-key") === cmd.key ? host : host.querySelector("[data-ui2-key='" + cmd.key + "']");
    if (!el) return;
    switch (cmd.op) {
      case "text":
        el.textContent =
          cmd.value && typeof cmd.value === "object" ? [cmd.value.loc].concat(cmd.value.args || []).join(" ") : String(cmd.value);
        break;
      case "cls":
        el.classList.toggle(cmd.name, !!cmd.on);
        break;
      case "style": {
        const v = cmd.value;
        const css = v === null ? "" : typeof v === "number" && !UNITLESS.has(cmd.prop) ? v + "px" : String(v);
        el.style.setProperty(cmd.prop, css);
        break;
      }
      case "show":
        el.style.display = cmd.on ? "" : "none";
        break;
    }
  }

  function take(msg) {
    if (!msg || msg.t !== "out") return;
    const changed = new Set();
    for (const s of msg.state || []) writeState(s.p, s.v, changed);
    for (const i of msg.intents || []) {
      shim.intents.push(i);
      console.info("ui2 shim: intent " + i.name, i.args);
    }
    for (const c of msg.cmds || []) if (!off) island(c);
    for (const s of msg.settings || []) {
      if (s.value === null) delete shim.settings[s.key];
      else shim.settings[s.key] = s.value;
    }
    for (const l of msg.log || []) {
      shim.log.push(l);
      const where = l.file ? " (" + l.file + ":" + l.line + ")" : "";
      const say = l.level === "error" ? console.error : l.level === "warn" ? console.warn : console.info;
      say.call(console, l.text + where);
    }
    if (changed.size) {
      sendFrame(changed, [], msg.n);
      render();
    }
  }

  // Answers are taken after the call that made them, as the game takes them in a later frame.
  function pump() {
    scheduled = false;
    while (inbox.length) take(inbox.shift());
  }

  const port = {
    post(msg) {
      inbox.push(msg);
      if (!scheduled) {
        scheduled = true;
        Promise.resolve().then(pump);
      }
    },
    now: () => (global.performance ? global.performance.now() : Date.now()),
  };

  // ---------------------------------------------------------------- the context

  setRoots(inlineSample());
  shim.ctx = api.createContext(
    {
      ctx: "skin",
      dev: true,
      env: {
        lang: "en_US",
        viewport: [global.innerWidth || 0, global.innerHeight || 0],
        scale: 1,
        cell: 0,
        skin: "mockup",
      },
      // The tables of the shim's model: whole roots, sent as they are.
      schema: {
        hash: null,
        parts: [{ base: 0, roots: ROOTS.map((n, i) => [i, n, "json"]), paths: [], types: {}, enums: {} }],
      },
      roots: shim.roots,
    },
    port,
  );
  shim.ui = shim.ctx.ui;
  global.ui = shim.ui;

  // ---------------------------------------------------------------- the markup's events

  function args(src) {
    const out = [];
    let named = null;
    for (const part of splitTop(src, ",")) {
      if (!part.trim()) continue;
      const m = /^\s*([A-Za-z_]\w*)\s*:(?!:)([\s\S]*)$/.exec(part);
      if (m && !/^\s*['"]/.test(part)) {
        named = named || {};
        named[m[1]] = evalLoose(m[2]);
      } else out.push(evalLoose(part));
    }
    if (named) return named;
    return out.length === 0 ? null : out.length === 1 ? out[0] : out;
  }

  // The value of an argument; what the shim cannot evaluate (a loop variable) is passed as its text.
  function evalLoose(src) {
    try {
      return compile(src.trim()).fn();
    } catch (e) {
      if (e instanceof Unknown) return src.trim();
      throw e;
    }
  }

  function splitTop(src, sep) {
    const parts = [];
    let depth = 0;
    let quote = null;
    let cur = "";
    for (const ch of src) {
      if (quote) {
        if (ch === quote) quote = null;
      } else if (ch === "'" || ch === '"') quote = ch;
      else if (ch === "(" || ch === "{") depth += 1;
      else if (ch === ")" || ch === "}") depth -= 1;
      else if (ch === sep && depth === 0) {
        parts.push(cur);
        cur = "";
        continue;
      }
      cur += ch;
    }
    parts.push(cur);
    return parts;
  }

  // Where an element is, for the scripts' error lines.
  function nodePath(el) {
    const bits = [];
    for (let e = el; e && e.nodeType === 1 && bits.length < 4; e = e.parentElement) {
      bits.unshift(e.localName + (e.id ? "#" + e.id : "") + (e.getAttribute("data-part") ? "[" + e.getAttribute("data-part") + "]" : ""));
    }
    return bits.join(" > ");
  }

  // Runs the actions of one element for one event; returns whether a `stop` ends the bubbling.
  function act(el, type, events, changed) {
    let stop = false;
    if (type === "click" && el.hasAttribute("data-event")) {
      const a = el.getAttribute("data-event-arg");
      events.push({ name: el.getAttribute("data-event"), arg: a === null ? null : evalLoose(inner(a)), node: nodePath(el) });
    }
    const src = el.getAttribute("data-on-" + type);
    if (src === null) return stop;
    for (const action of splitTop(src, ";")) {
      const s = action.trim();
      let m;
      if (!s) continue;
      if (s === "stop") stop = true;
      else if ((m = /^call\s+([\w.-]+)\s*\(([\s\S]*)\)$/.exec(s))) events.push({ name: m[1], arg: args(m[2]), node: nodePath(el) });
      else if ((m = /^intent\s+([\w.-]+)\s*\(([\s\S]*)\)$/.exec(s))) {
        const i = { name: m[1], args: args(m[2]) };
        shim.intents.push(i);
        console.info("ui2 shim: intent " + i.name, i.args);
      } else if ((m = /^toggle\s+([\w.]+)$/.exec(s))) writeState(m[1], !get(m[1]), changed);
      else if ((m = /^set\s+([\w.]+)\s*=([\s\S]+)$/.exec(s))) writeState(m[1], evalLoose(m[2]), changed);
    }
    return stop;
  }

  function onEvent(ev) {
    const own = OWN_EVENTS.includes(ev.type);
    const events = [];
    const changed = new Set();
    for (let el = ev.target; el && el.nodeType === 1; el = el.parentElement) {
      if (act(el, ev.type, events, changed) || own) break;
    }
    if (events.length || changed.size) {
      sendFrame(changed, events);
      render();
    }
  }

  function onKey(ev) {
    const code = (ev.ctrlKey ? "Ctrl+" : "") + (ev.altKey ? "Alt+" : "") + (ev.shiftKey ? "Shift+" : "") + ev.code;
    for (const el of doc.querySelectorAll("[data-hotkey]")) {
      if (el.getAttribute("data-hotkey") === code) {
        ev.preventDefault();
        el.click();
        return;
      }
    }
  }

  function start() {
    if (off) return;
    for (const type of EVENTS.concat(OWN_EVENTS)) doc.addEventListener(type, onEvent, true);
    doc.addEventListener("keydown", onKey);
    render();
    shim.ctx.ready();
  }

  // The sample of a served page arrives later: the scripts see it as a change of the whole model.
  function fetchSample() {
    if (inlineSample() || !global.fetch || !global.location || global.location.protocol === "file:") {
      if (!inlineSample() && global.location && global.location.protocol === "file:") {
        console.info("ui2 shim: from file:// the sample is not fetched; set window.__ui2sample or a data-ui2-sample element");
      }
      return Promise.resolve();
    }
    return global
      .fetch("sample/" + sampleName + ".json")
      .then((r) => (r.ok ? r.json() : null))
      .then((s) => {
        if (!s) return;
        setRoots(s);
        sendFrame(new Set(ROOTS.filter((r) => shim.roots[r] !== undefined)), []);
        render();
      })
      .catch((e) => console.info("ui2 shim: no sample " + sampleName + ": " + e.message));
  }

  shim.ready = new Promise((resolve) => {
    const go = () => {
      // A data-ui2-sample element below the shim is there now.
      const late = !global.__ui2sample && inlineSample();
      if (late) {
        setRoots(late);
        sendFrame(new Set(ROOTS.filter((r) => shim.roots[r] !== undefined)), []);
      }
      start();
      fetchSample().then(resolve);
    };
    if (!doc || doc.readyState !== "loading") go();
    else doc.addEventListener("DOMContentLoaded", go, { once: true });
  });
})(globalThis);
