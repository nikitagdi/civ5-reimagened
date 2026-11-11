// The `ui` object of skin scripts (api.js) and the worker loader (worker.js), without a browser:
// node --test "apps/web/www/ui2/api.test.mjs"
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import vm from "node:vm";

const API_SRC = readFileSync(new URL("./api.js", import.meta.url), "utf8");
const WORKER_SRC = readFileSync(new URL("./worker.js", import.meta.url), "utf8");

vm.runInThisContext(API_SRC, { filename: "api.js" });
const api = globalThis.__ui2api;

// Tables in the form the supervisor sends (ui2-script `proto.rs`): a small schema and a state fragment.
// Field ids: top 0, top.gold 1, top.res 2, top.res[].id 3, top.res[].n 4, ui 5, ui.tab 6, ui.sel 7,
// notes 8, notes[].id 9, notes[].text 10; state 11, state.sort 12, state.open 13.
const SCHEMA = {
  hash: "00000000000000ab",
  parts: [
    {
      base: 0,
      roots: [
        [0, "top", ["struct", "Top"]],
        [1, "ui", ["struct", "Ui"]],
        [2, "notes", ["list", "Note"]],
      ],
      enums: { Tab: ["all", "units"] },
      types: {
        Res: [["id", "key"], ["n", "i32"]],
        Top: [["gold", "i32"], ["res", ["list", "Res"]]],
        Ui: [["tab", ["enum", "Tab"]], ["sel", "i32"]],
        Note: [["id", "key"], ["text", "str"]],
      },
      paths: [
        ["top", ["struct", "Top"], null, false],
        ["top.gold", "i32", null, false],
        ["top.res", ["list", "Res"], "id", false],
        ["top.res[].id", "key", null, false],
        ["top.res[].n", "i32", null, false],
        ["ui", ["struct", "Ui"], null, false],
        ["ui.tab", ["enum", "Tab"], null, true],
        ["ui.sel", "i32", null, false],
        ["notes", ["list", "Note"], "id", false],
        ["notes[].id", "key", null, false],
        ["notes[].text", "str", null, false],
      ],
    },
    {
      base: 11,
      roots: [[3, "state", ["struct", "state.Root"]]],
      enums: {},
      types: { "state.Root": [["sort", "str"], ["open", "bool"]] },
      paths: [
        ["state", ["struct", "state.Root"], null, false],
        ["state.sort", "str", null, false],
        ["state.open", "bool", null, false],
      ],
    },
  ],
};

const SNAPSHOT = [
  1,
  [5, "00000000000000ab"],
  [4, 0, [10, [["0:1", ["0:1", 3]], ["0:2", ["0:2", 4]]]]],
  [4, 1, [0, 0]],
  [4, 3, ["name", false]],
];

function init(extra = {}) {
  return Object.assign(
    {
      t: "init",
      api: "1.0.0",
      ctx: "skin",
      files: [],
      model: SNAPSHOT,
      schema: SCHEMA,
      reads: ["top", "ui", "state"],
      ns: { state: "state", intents: null },
      settings: [],
      env: { lang: "en_US", viewport: [800, 600], scale: 1, cell: 0, skin: "t" },
      dev: false,
    },
    extra,
  );
}

function context(extra) {
  const sent = [];
  let clock = 0;
  const port = {
    post: (m) => sent.push(JSON.parse(JSON.stringify(m))),
    now: () => clock,
  };
  const ctx = api.createContext(init(extra), port);
  return { ctx, ui: ctx.ui, sent, tick: (ms) => (clock += ms) };
}

let seq = 0;
function frame(model, events = [], applied = 0) {
  seq += 1;
  return { t: "frame", seq, applied, model, events };
}

test("the snapshot is decoded: structs, enums, keyed lists, the state fragment", () => {
  const { ui } = context();
  assert.equal(ui.api, "1.0.0");
  assert.equal(ui.model.get("top.gold"), 10);
  assert.deepEqual(JSON.parse(JSON.stringify(ui.model.get("top.res"))), [
    { id: "0:1", n: 3 },
    { id: "0:2", n: 4 },
  ]);
  assert.equal(ui.model.get("ui.tab"), "all");
  assert.equal(ui.state.get("sort"), "name");
  assert.equal(ui.env.lang, "en_US");
});

test("reading outside the reads throws", () => {
  const { ui } = context();
  assert.throws(() => ui.model.get("notes"), /outside the model this script reads/);
  assert.throws(() => ui.model.on("notes", () => {}), /outside/);
});

test("subscriptions fold into one call per batch", () => {
  const { ctx, ui, sent } = context();
  const calls = { top: 0, gold: 0, ui: 0 };
  let seen = null;
  ui.model.on("top", () => calls.top++);
  ui.model.on("top.gold", (v, old) => {
    calls.gold++;
    seen = [v, old];
  });
  ui.model.on("ui", () => calls.ui++);
  const f = frame([2, [0, 1, [], 11], [0, 4, ["0:1"], 5], [0, 1, [], 12]]);
  ctx.receive(f);
  assert.deepEqual(calls, { top: 1, gold: 1, ui: 0 });
  assert.deepEqual(seen, [12, 10]);
  assert.equal(ui.model.get("top.res")[0].n, 5);
  // Every frame is answered, even with nothing to say.
  assert.equal(sent.length, 1);
  assert.equal(sent[0].t, "out");
  assert.equal(sent[0].ack, f.seq);
  // A batch that does not touch a path calls nothing.
  ctx.receive(frame([3, [0, 13, [], true]]));
  assert.deepEqual(calls, { top: 1, gold: 1, ui: 0 });
});

test("onList reports added, changed and removed elements by key", () => {
  const { ctx, ui } = context();
  const log = [];
  ui.model.onList("top.res", {
    add: (k, v) => log.push(["add", k, v.n]),
    change: (k, v) => log.push(["change", k, v.n]),
    remove: (k) => log.push(["remove", k]),
  });
  ctx.receive(
    frame([
      2,
      [1, 2, [], 0, "0:3", ["0:3", 9]],
      [0, 4, ["0:2"], 40],
      [0, 4, ["0:2"], 41],
      [2, 2, [], "0:1"],
    ]),
  );
  assert.deepEqual(log, [
    ["add", "0:3", 9],
    ["change", "0:2", 41],
    ["remove", "0:1"],
  ]);
  assert.deepEqual(
    ui.model.get("top.res").map((r) => r.id),
    ["0:3", "0:2"],
  );
  log.length = 0;
  ctx.receive(frame([3, [3, 2, [], "0:3", 1]]));
  assert.deepEqual(log, [], "a move changes no element");
  assert.deepEqual(
    ui.model.get("top.res").map((r) => r.id),
    ["0:2", "0:3"],
  );
  assert.throws(() => ui.model.onList("top.res[].n", {}), /inside a list/);
});

test("an exception switches off only its own handler", () => {
  const { ctx, ui, sent } = context();
  let other = 0;
  let bad = 0;
  ui.on("sort", () => {
    bad++;
    throw new Error("boom");
  });
  ui.on("sort", () => other++);
  ctx.receive(frame(null, [{ name: "sort", arg: null, node: "hud.html:3" }]));
  ctx.receive(frame(null, [{ name: "sort", arg: null, node: "hud.html:3" }]));
  assert.equal(bad, 1);
  assert.equal(other, 2);
  const errors = sent.flatMap((m) => m.log).filter((l) => l.level === "error");
  assert.equal(errors.length, 1);
  assert.match(errors[0].text, /^ui2\.js: skin on\(sort\) disabled: boom/);
});

test("a slow handler is reported once and kept", () => {
  const { ctx, ui, sent, tick } = context();
  let n = 0;
  ui.on("tick", () => {
    n++;
    tick(80);
  });
  ctx.receive(frame(null, [{ name: "tick", arg: null, node: "" }]));
  ctx.receive(frame(null, [{ name: "tick", arg: null, node: "" }]));
  assert.equal(n, 2);
  const warns = sent.flatMap((m) => m.log).filter((l) => l.level === "warn");
  assert.equal(warns.length, 1);
  assert.match(warns[0].text, /slow handler: 80 ms/);
  assert.equal(sent[0].busy_ms, 80);
});

test("in development mode the mirror is frozen", () => {
  const { ctx, ui } = context({ dev: true });
  assert.throws(() => {
    ui.model.get("top").gold = 1;
  }, TypeError);
  ctx.receive(frame([2, [0, 1, [], 20]]));
  assert.throws(() => {
    ui.model.get("top.res").push({});
  }, TypeError);
  assert.equal(ui.model.get("top.gold"), 20);
});

test("state writes go out and are read back until the model has them", () => {
  const { ctx, ui, sent } = context();
  ui.on("go", () => {
    ui.state.set("sort", "turns");
    ui.state.merge("", { open: true });
    ui.state.set("ui.tab", "units");
  });
  ctx.receive(frame(null, [{ name: "go", arg: null, node: "" }]));
  const out = sent.at(-1);
  assert.deepEqual(out.state, [
    { p: "state.sort", v: "turns" },
    { p: "state.open", v: true },
    { p: "ui.tab", v: "units" },
  ]);
  assert.equal(ui.state.get("sort"), "turns", "read your own write");
  // The model has not caught up yet, and says so: the overlay stays.
  ctx.receive(frame(null, [], 0));
  assert.equal(ui.state.get("sort"), "turns");
  // The supervisor applied that out; the model is the truth again (here the write was refused).
  ctx.receive(frame(null, [], out.n));
  assert.equal(ui.state.get("sort"), "name");
  assert.throws(() => ui.state.set("ui.sel", 1), /not a field of `ui` scripts may write/);
});

test("state.on sees the model", () => {
  const { ctx, ui } = context();
  const seen = [];
  ui.state.on("open", (v) => seen.push(v));
  ctx.receive(frame([2, [0, 13, [], true]]));
  assert.deepEqual(seen, [true]);
});

test("intents go out; a mod keeps to its prefix", () => {
  const skin = context();
  skin.ui.intent("turn.primary");
  skin.ui.intent("unit.select", { unit: "0:7" });
  skin.ctx.ready();
  assert.deepEqual(skin.sent[0].intents, [
    { name: "turn.primary", args: {} },
    { name: "unit.select", args: { unit: "0:7" } },
  ]);
  assert.equal(skin.sent[0].ack, null);

  const mod = context({ ctx: "mod:hello", ns: { state: "modstate.hello", intents: "mods.hello." } });
  mod.ui.intent("turn.primary");
  mod.ui.intent("mods.hello.poke", { n: 1 });
  mod.ctx.ready();
  assert.deepEqual(mod.sent[0].intents, [{ name: "mods.hello.poke", args: { n: 1 } }]);
  assert.match(mod.sent[0].log[0].text, /intent `turn\.primary` is not one of `mods\.hello\.\*`/);
});

test("events and env reach ui.on handlers", () => {
  const { ctx, ui } = context();
  const got = [];
  ui.on("pick", (arg, info) => got.push([arg, info.node]));
  ui.on("env", (env) => got.push(["env", env.viewport[0]]));
  ctx.receive(Object.assign(frame(null, [{ name: "pick", arg: "1:2", node: "hud.html:9" }]), { env: { viewport: [1024, 768] } }));
  assert.deepEqual(got, [
    ["env", 1024],
    ["1:2", "hud.html:9"],
  ]);
  assert.equal(ui.env.viewport[1], 768);
  assert.deepEqual(ctx.handlers(), ["pick", "env"]);
});

test("ready runs the ready handlers; an empty out is not sent", () => {
  const { ctx, ui, sent } = context();
  let ready = 0;
  ui.on("ready", () => ready++);
  ctx.ready();
  assert.equal(ready, 1);
  assert.equal(sent.length, 0);
});

test("log lines are limited per second", () => {
  const { ctx, ui, sent, tick } = context();
  for (let i = 0; i < 30; i++) ui.log.info("line", i);
  ctx.ready();
  assert.equal(sent[0].log.length, 20);
  assert.equal(sent[0].log[0].text, "line 0");
  tick(1000);
  ui.log.warn({ a: 1 });
  ctx.ready();
  assert.deepEqual(
    sent[1].log.map((l) => [l.level, l.text]),
    [["warn", '{"a":1}']],
  );
});

// ------------------------------------------------------------------ worker.js in a fake worker global

function workerGlobal() {
  const posted = [];
  const listeners = {};
  const host = {
    post: (text) => posted.push(JSON.parse(text)),
    close: () => posted.push({ t: "closed" }),
    listen: (type, fn) => (listeners[type] ||= []).push(fn),
    load: (name) => {
      assert.equal(name, "api.js");
      vm.runInContext(API_SRC, g, { filename: "api.js" });
    },
  };
  const g = vm.createContext({ __host: host });
  // The chain of a dedicated worker: own members, then the prototypes of the worker scopes.
  vm.runInContext(
    `(() => {
      const h = __host;
      delete globalThis.__host;
      const scope = { fetch() {}, indexedDB: {}, caches: {}, addEventListener: h.listen };
      Object.setPrototypeOf(scope, Object.getPrototypeOf(globalThis));
      const dedicated = Object.create(scope);
      dedicated.postMessage = h.post;
      dedicated.close = h.close;
      Object.setPrototypeOf(globalThis, dedicated);
      Object.assign(globalThis, {
        self: globalThis,
        importScripts: h.load,
        WebSocket: function WebSocket() {},
        XMLHttpRequest: function XMLHttpRequest() {},
        Worker: function Worker() {},
        BroadcastChannel: function BroadcastChannel() {},
        navigator: Object.create({ storage: {}, userAgent: "test" }),
      });
    })()`,
    g,
  );
  vm.runInContext(WORKER_SRC, g, { filename: "worker.js" });
  const send = (msg) => {
    for (const fn of listeners.message || []) fn({ data: JSON.stringify(msg) });
  };
  return { g, posted, send, listeners, run: (code) => vm.runInContext(code, g) };
}

test("the worker takes the forbidden globals away before any skin code runs", () => {
  const w = workerGlobal();
  for (const name of [
    "fetch",
    "XMLHttpRequest",
    "WebSocket",
    "importScripts",
    "indexedDB",
    "caches",
    "Worker",
    "BroadcastChannel",
    "postMessage",
    "close",
    "__ui2api",
  ]) {
    assert.equal(w.run("typeof self." + name), "undefined", name);
    assert.equal(w.run("typeof Object.getPrototypeOf(self)." + name), "undefined", name + " on the prototype");
  }
  assert.equal(w.run("typeof fetch"), "undefined");
  assert.equal(w.run("typeof navigator.storage"), "undefined");
  assert.equal(w.run("navigator.userAgent"), "test");
});

test("the worker runs the files in strict mode and reports ready", () => {
  const w = workerGlobal();
  w.send(
    init({
      files: [
        {
          path: "js/a.js",
          code: [
            "ui.on('sort', (k) => ui.state.set('sort', k));",
            "ui.on('ready', () => ui.intent('turn.primary'));",
            "ui.state.set('open', typeof fetch === 'undefined' && this === undefined);",
            "self.shared = 1;",
          ].join("\n"),
        },
        { path: "js/b.js", code: "ui.state.set('sort', 'b' + self.shared);" },
      ],
    }),
  );
  assert.equal(w.posted[0].t, "ready");
  assert.deepEqual(w.posted[0].handlers, ["sort", "ready"]);
  const out = w.posted[1];
  assert.equal(out.t, "out");
  assert.equal(out.ack, null);
  assert.deepEqual(out.state, [
    { p: "state.open", v: true },
    { p: "state.sort", v: "b1" },
  ]);
  assert.deepEqual(out.intents, [{ name: "turn.primary", args: {} }]);
  w.send({ t: "frame", seq: 1, applied: 1, model: null, events: [{ name: "sort", arg: "turns", node: "" }] });
  assert.deepEqual(w.posted[2].state, [{ p: "state.sort", v: "turns" }]);
  assert.equal(w.posted[2].ack, 1);
  w.send({ t: "stop" });
  assert.equal(w.posted[3].t, "closed");
});

test("a file that throws fails the context with its file and line", () => {
  const w = workerGlobal();
  w.send(
    init({
      files: [
        { path: "js/ok.js", code: "ui.on('x', () => {});" },
        { path: "js/bad.js", code: "// one\n// two\nnope.x = 1;\n" },
      ],
    }),
  );
  assert.equal(w.posted.length, 1);
  assert.equal(w.posted[0].t, "fail");
  assert.deepEqual(w.posted[0].error, { message: "nope is not defined", file: "js/bad.js", line: 3 });
  // A failed context ignores frames.
  w.send({ t: "frame", seq: 1, model: null, events: [] });
  assert.equal(w.posted.length, 1);
});

test("a handler error names the file and line", () => {
  const w = workerGlobal();
  w.send(init({ files: [{ path: "js/h.js", code: "\nui.on('go', () => {\n  missing();\n});" }] }));
  w.send({ t: "frame", seq: 1, model: null, events: [{ name: "go", arg: null, node: "" }] });
  const out = w.posted.at(-1);
  assert.equal(out.log.length, 1);
  assert.equal(out.log[0].file, "js/h.js");
  assert.equal(out.log[0].line, 3);
  assert.match(out.log[0].text, /^ui2\.js: skin on\(go\) disabled: missing is not defined at js\/h\.js:3$/);
});

test("error lines carry the key the supervisor writes them once by, and the element of the event", () => {
  const w = workerGlobal();
  w.send(init({ files: [{ path: "js/h.js", code: "ui.on('go', () => { throw new Error('boom'); });" }] }));
  w.send({ t: "frame", seq: 1, model: null, events: [{ name: "go", arg: null, node: "hud.html/div[2]" }] });
  const l = w.posted.at(-1).log[0];
  assert.equal(l.key, "on(go)|boom");
  assert.match(l.text, /disabled: boom at js\/h\.js:1 \(element hud\.html\/div\[2\]\)$/);
});

test("an uncaught error of the worker becomes one log line, not a failed worker", () => {
  const w = workerGlobal();
  w.send(init({ files: [{ path: "js/a.js", code: "self.later = () => { throw new Error('late'); };" }] }));
  const errorEvent = { error: (() => { try { w.run("later()"); } catch (e) { return e; } })(), message: "late", cancelled: false };
  errorEvent.preventDefault = () => (errorEvent.cancelled = true);
  for (const fn of w.listeners.error || []) fn(errorEvent);
  assert.equal(errorEvent.cancelled, true);
  const l = w.posted.at(-1).log[0];
  assert.equal(l.level, "error");
  assert.equal(l.key, "uncaught error|late");
  assert.match(l.text, /^ui2\.js: skin uncaught error: late at js\/a\.js:1$/);
});