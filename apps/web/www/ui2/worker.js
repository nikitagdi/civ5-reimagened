// The loader of a UI v2 script worker (a classic worker script, one worker per script context).
//
// It loads `api.js`, takes away what skin scripts must not use (network, storage, more workers, the raw
// message port), and waits for the `init` message of the supervisor (the ui2-script crate): then it runs the
// files of the skin or mod in order, each as `function (ui) { 'use strict'; <file> }` with the source URL
// `ui2:/<skin>/<path>` so errors point at the file and line, and answers `ready` (or `fail` if a file threw).
// Later messages (`frame`) go to the context of `api.js`; `stop` closes the worker.
//
// Scripts are trusted, like mods: taking the globals away keeps a skin from depending on the network or the
// page by accident; it is not a sandbox. The server also sends this file with a CSP that blocks the network.
(function (g) {
  "use strict";

  const has = (o, k) => Object.prototype.hasOwnProperty.call(o, k);

  // Remove `names` from `obj` and every object on its prototype chain; a property that cannot be deleted is
  // pinned to `undefined`.
  function strip(obj, names) {
    for (const name of names) {
      for (let o = obj; o && o !== Object.prototype; o = Object.getPrototypeOf(o)) {
        if (!has(o, name)) continue;
        try {
          delete o[name];
        } catch (_) {
          // Not configurable: pinned below.
        }
        if (has(o, name)) {
          try {
            Object.defineProperty(o, name, { value: undefined, writable: false, configurable: false });
          } catch (_) {
            // Nothing more can be done for it.
          }
        }
      }
    }
  }

  // Taken before anything is removed, kept in this closure only.
  const post = g.postMessage.bind(g);
  const close = g.close.bind(g);
  const listen = g.addEventListener.bind(g);
  const clock = g.performance && g.performance.now ? () => g.performance.now() : () => Date.now();

  g.importScripts("api.js");
  const api = g.__ui2api;
  strip(g, [
    "__ui2api",
    "fetch",
    "XMLHttpRequest",
    "WebSocket",
    "WebSocketStream",
    "EventSource",
    "WebTransport",
    "importScripts",
    "Worker",
    "SharedWorker",
    "BroadcastChannel",
    "MessageChannel",
    "indexedDB",
    "caches",
    "localStorage",
    "sessionStorage",
    "postMessage",
    "onmessage",
    "onmessageerror",
    "close",
  ]);
  if (g.navigator) strip(g.navigator, ["storage"]);

  // SIMPLIFIED(SIMP-261): skin files are classic scripts sharing the scope of their context, no ES modules
  // (`import`/`export`); code shared between files goes through globals of the worker.
  function compile(code, skin, path) {
    return new Function("ui", "'use strict';\n" + code + "\n//# sourceURL=ui2:/" + skin + "/" + path);
  }

  // How many lines the wrapper of `new Function` puts before the first line of a file in this engine.
  try {
    compile("throw new Error('probe');", "probe", "probe.js")(null);
  } catch (e) {
    const at = api.locate(e);
    if (at.line) api.setLineOffset(at.line - 1);
  }

  const port = {
    post(msg) {
      post(JSON.stringify(msg));
    },
    now: clock,
  };
  let ctx = null;

  function init(msg) {
    const t0 = clock();
    const fail = (message, file, line) => {
      ctx = null;
      port.post({ t: "fail", ctx: msg.ctx, error: { message, file: file || null, line: line || null } });
    };
    let c;
    try {
      c = api.createContext(msg, port);
    } catch (e) {
      fail("the context could not start: " + (e && e.message ? e.message : String(e)));
      return;
    }
    const skin = (msg.env && msg.env.skin) || "skin";
    for (const f of msg.files || []) {
      try {
        compile(f.code, skin, f.path).call(undefined, c.ui);
      } catch (e) {
        const at = api.locate(e);
        fail(e && e.message ? String(e.message) : String(e), at.file || f.path, at.line);
        return;
      }
    }
    ctx = c;
    port.post({ t: "ready", ctx: msg.ctx, handlers: c.handlers(), ms: clock() - t0 });
    c.ready();
  }

  listen("message", (e) => {
    let msg;
    try {
      msg = JSON.parse(e.data);
    } catch (_) {
      return;
    }
    if (msg.t === "init") init(msg);
    else if (msg.t === "stop") close();
    else if (ctx) ctx.receive(msg);
  });

  listen("unhandledrejection", (e) => {
    if (e.preventDefault) e.preventDefault();
    if (ctx) ctx.rejection(e.reason);
  });
})(self);
