// A worker of the jobs pool (B3 render-mt): instantiates the threaded build on the page's module and
// shared memory, then runs the pool's loop (it never returns; the GPU stays on the page's thread).
//
// A trap here (a panic aborts, an allocation fails, a stack overflows) terminates the whole module: the
// flag of wasm-bindgen sits in the shared memory, so the page only sees "Module terminated" after it. The
// first error goes to the page (`{ civ5trap }`), which puts it in its crash report (`bootstrap.js`).
let reported = false;
// The Rust panic hook hands its text here before the worker aborts (`apps/web` `note_worker_panics`).
let panicText = null;
self.__civ5OnPanic = (text) => {
  panicText ??= String(text).slice(0, 1500);
};
const report = (e, where, sign = false) => {
  // The first error only (the glue's hook and the catch below see the same trap).
  if (reported) return;
  reported = true;
  try {
    self.postMessage({
      civ5trap: {
        sign,
        panic: panicText,
        where: `${self.name || "jobs worker"}: ${where}`,
        message: e && e.message ? String(e.message) : String(e),
        name: e && e.name ? String(e.name) : typeof e,
        stack: e && e.stack ? String(e.stack) : "",
      },
    });
  } catch (_) {}
};
// The glue calls this the moment it marks the module terminated (patched in by `cargo xtask`, SIMP-exe-690).
// `already`: another thread had terminated it (this thread was only stopped at its next call into JS).
self.__civ5OnTrap = (e, already) =>
  report(e, already ? "stopped (the module was already terminated)" : "wasm trap", already === true);
self.addEventListener("error", (ev) => report(ev.error ?? ev.message, "error event"));
self.addEventListener("unhandledrejection", (ev) => report(ev.reason, "unhandled rejection"));

self.onmessage = async ({ data }) => {
  try {
    const glue = await import("./pkg-mt/app_web.js");
    await glue.default({ module_or_path: data.module, memory: data.memory });
    glue.jobs_worker_main();
  } catch (e) {
    report(e, "jobs_worker_main");
  }
};
