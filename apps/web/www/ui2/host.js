// The script workers of UI v2 skins and mods, page side (web only).
//
// Contract with the wasm (platform-web, the `ScriptHost` implementation):
//   window.__civ5ui2.spawn()          -> id (a number) of a new worker running `ui2/worker.js`; throws if the page
//                                        cannot create workers
//   window.__civ5ui2.post(id, text)   -> true if the worker exists; texts arrive in order
//   window.__civ5ui2.drain()          -> [[id, "msg" | "error", text], ...] since the last call, oldest first
//                                        ("error": the worker itself failed and is of no further use)
//   window.__civ5ui2.terminate(id)    stops the worker at once and drops its undrained messages
// Messages are queued here until the frame drains them, like the page screens of `ui.js`.
(function () {
  "use strict";

  const script = document.currentScript;
  const url = new URL("worker.js", script && script.src ? script.src : new URL("ui2/", document.baseURI)).href;
  const workers = new Map();
  let next = 0;
  let queue = [];

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
    drain() {
      if (!queue.length) return [];
      const q = queue;
      queue = [];
      return q;
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
