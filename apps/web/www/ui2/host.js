// The script workers of UI v2 skins and mods, page side (web only).
//
// Contract with the wasm (platform-web, the `ScriptHost` implementation):
//   window.__civ5ui2.spawn()          -> id (a number) of a new worker running `ui2/worker.js`; throws if the page
//                                        cannot create workers
//   window.__civ5ui2.post(id, text)   -> true if the worker exists; texts arrive in order
//   window.__civ5ui2.drain(ids)       -> [[id, "msg" | "error", text], ...] of the workers `ids` since the last call,
//                                        oldest first; the other workers' messages stay queued (each script host of
//                                        the wasm, the skin's and the mods', drains its own workers). Without `ids`:
//                                        every worker's. ("error": the worker itself failed and is of no further use)
//   window.__civ5ui2.terminate(id)    stops the worker at once and drops its undrained messages
//   window.__civ5ui2.live()           -> the time (`performance.now()`) up to which the page has taken in what the
//                                        workers posted: a message posted before it is in the queue (or was
//                                        drained). The page's main thread may hold the workers' messages back for
//                                        seconds while it has input to handle and long frames (the browser runs
//                                        input and rendering first); the watchdog of the wasm counts a worker's
//                                        wait up to this time, not up to now. null: the page cannot tell.
// Messages are queued here until the frame drains them, like the page screens of `ui.js`.
(function () {
  "use strict";

  const script = document.currentScript;
  const url = new URL("worker.js", script && script.src ? script.src : new URL("ui2/", document.baseURI)).href;
  const workers = new Map();
  let next = 0;
  let queue = [];

  // `live()`: a ping on a channel of our own, posted while none is on its way. A worker's message and a port's
  // message are tasks of the same kind (posted messages), run in the order they were queued: when the ping runs,
  // every worker message queued before it was posted has run too, so `delivered` is the time the last ping that
  // ran was posted. Nothing was posted to this page before this file ran.
  const channel = typeof MessageChannel === "function" ? new MessageChannel() : null;
  let delivered = performance.now();
  let pingAt = null;
  if (channel) {
    channel.port1.onmessage = () => {
      delivered = pingAt;
      pingAt = null;
    };
  }
  function ping() {
    if (!channel || pingAt !== null) return;
    pingAt = performance.now();
    channel.port2.postMessage(0);
  }

  window.__civ5ui2 = {
    spawn() {
      const id = next++;
      const w = new Worker(url);
      w.onmessage = (e) => queue.push([id, "msg", String(e.data)]);
      w.onerror = (e) => {
        e.preventDefault();
        const where = e.filename ? " at " + e.filename + ":" + e.lineno : "";
        queue.push([id, "error", (e.message || "worker error") + where]);
      };
      w.onmessageerror = () => queue.push([id, "error", "a message of the worker could not be read"]);
      workers.set(id, w);
      return id;
    },
    post(id, text) {
      const w = workers.get(id);
      if (!w) return false;
      w.postMessage(text);
      return true;
    },
    live() {
      ping();
      return channel ? delivered : null;
    },
    drain(ids) {
      ping();
      if (!queue.length) return [];
      if (ids === undefined) {
        const q = queue;
        queue = [];
        return q;
      }
      const want = new Set(ids);
      const mine = [];
      const rest = [];
      for (const m of queue) (want.has(m[0]) ? mine : rest).push(m);
      queue = rest;
      return mine;
    },
    terminate(id) {
      const w = workers.get(id);
      if (!w) return;
      w.terminate();
      workers.delete(id);
      queue = queue.filter((m) => m[0] !== id);
    },
  };
})();
