// The main-thread arm of the script host measurement (bench.html, playtest scenario `ui2-n6`).
//
// A script host with the contract of `ui2/host.js` (`spawn`, `post`, `drain`, `terminate`), so the same supervisor
// (the Rust one of `Ui2ScriptSelftest`, or the bench's own loop) drives either arm unchanged. Here a "worker" is an
// `api.js` context in the page itself: `post` handles the message at once, on the page's thread, so the skin's
// handlers run inside the frame that posts; their answers wait in a queue for the next `drain`, as a worker's would.
// The texts are the same JSON as on the worker path, so both arms pay the same encoding.
//
// Nothing can interrupt a handler here: a script that hangs stalls the page until it returns.

/** The `ui2/api.js` module of the page (loaded by a classic <script> before this module). */
function api() {
  const a = globalThis.__ui2api;
  if (!a) throw new Error("ui2/api.js is not loaded");
  return a;
}

/** Compile a skin file the way `ui2/worker.js` does: strict, with a source URL naming the skin and the file. */
function compile(code, skin, path) {
  return new Function("ui", "'use strict';\n" + code + "\n//# sourceURL=ui2:/" + skin + "/" + path);
}

let lineOffsetSet = false;

function probeLineOffset(a) {
  if (lineOffsetSet) return;
  lineOffsetSet = true;
  try {
    compile("throw new Error('probe');", "probe", "probe.js")(null);
  } catch (e) {
    const at = a.locate(e);
    if (at.line) a.setLineOffset(at.line - 1);
  }
}

/**
 * A script host whose contexts run on the page's main thread.
 * Extra fields for the bench: `stats()` -> {posts, handledMs} (time spent inside `post`, i.e. in the scripts).
 */
export function createMainThreadHost() {
  const a = api();
  probeLineOffset(a);
  const slots = new Map();
  let next = 0;
  let queue = [];
  let posts = 0;
  let handledMs = 0;
  const clock = () => performance.now();

  function init(slot, id, msg) {
    const t0 = clock();
    const port = {
      post(m) {
        if (slot.alive) queue.push([id, "msg", JSON.stringify(m)]);
      },
      now: clock,
    };
    const fail = (message, file, line) => {
      slot.ctx = null;
      port.post({ t: "fail", ctx: msg.ctx, error: { message, file: file || null, line: line || null } });
    };
    let c;
    try {
      c = a.createContext(msg, port);
    } catch (e) {
      fail("the context could not start: " + (e && e.message ? e.message : String(e)));
      return;
    }
    const skin = (msg.env && msg.env.skin) || "skin";
    for (const f of msg.files || []) {
      try {
        compile(f.code, skin, f.path).call(undefined, c.ui);
      } catch (e) {
        const at = a.locate(e);
        fail(e && e.message ? String(e.message) : String(e), at.file || f.path, at.line);
        return;
      }
    }
    slot.ctx = c;
    port.post({ t: "ready", ctx: msg.ctx, handlers: c.handlers(), ms: clock() - t0 });
    c.ready();
  }

  return {
    spawn() {
      const id = next++;
      slots.set(id, { alive: true, ctx: null });
      return id;
    },
    post(id, text) {
      const slot = slots.get(id);
      if (!slot) return false;
      const t0 = clock();
      posts += 1;
      let msg = null;
      try {
        msg = JSON.parse(text);
      } catch (_) {
        // A text that is not JSON is dropped, as the worker loader does.
      }
      if (msg && msg.t === "init") init(slot, id, msg);
      else if (msg && msg.t === "stop") {
        slot.alive = false;
        slots.delete(id);
      } else if (msg && slot.ctx) slot.ctx.receive(msg);
      handledMs += clock() - t0;
      return true;
    },
    drain() {
      if (!queue.length) return [];
      const q = queue;
      queue = [];
      return q;
    },
    terminate(id) {
      const slot = slots.get(id);
      if (!slot) return;
      // Timers of the context may still fire: their answers are dropped.
      slot.alive = false;
      slots.delete(id);
      queue = queue.filter((m) => m[0] !== id);
    },
    stats() {
      return { posts, handledMs };
    },
  };
}
