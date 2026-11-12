// The page side of the script workers (host.js), without a browser: a fake `Worker` stands in for the real one.
// node --test "apps/web/www/ui2/host.test.mjs"
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import vm from "node:vm";

const HOST_SRC = readFileSync(new URL("./host.js", import.meta.url), "utf8");

/** A value of the page's realm as a plain one of this realm (arrays of another realm are not deep-equal). */
const plain = (v) => JSON.parse(JSON.stringify(v));

/** A page with host.js loaded; `workers` are the fake workers in the order they were made. */
function page() {
  const workers = [];
  class Worker {
    constructor(url) {
      this.url = url;
      this.posted = [];
      this.terminated = false;
      workers.push(this);
    }
    postMessage(text) {
      this.posted.push(text);
    }
    terminate() {
      this.terminated = true;
    }
    /** What the worker posts to the page. */
    send(text) {
      this.onmessage({ data: text });
    }
  }
  const window = {};
  const ctx = vm.createContext({
    window,
    Worker,
    URL,
    document: { currentScript: { src: "http://127.0.0.1/ui2/host.js" }, baseURI: "http://127.0.0.1/" },
  });
  vm.runInContext(HOST_SRC, ctx, { filename: "host.js" });
  return { host: window.__civ5ui2, workers };
}

test("a drain with ids takes only those workers' messages and leaves the others queued", () => {
  // Two owners on one page (the skin's supervisor and the mods'), each with its own worker.
  const { host, workers } = page();
  const skin = host.spawn();
  const mod = host.spawn();
  assert.notEqual(skin, mod);
  workers[0].send("skin ready");
  workers[1].send("mod ready");
  workers[0].send("skin out");
  // The skin's owner drains first: the mod's answer is not its to take.
  assert.deepEqual(plain(host.drain([skin])), [
    [skin, "msg", "skin ready"],
    [skin, "msg", "skin out"],
  ]);
  assert.deepEqual(plain(host.drain([mod])), [[mod, "msg", "mod ready"]]);
  assert.deepEqual(plain(host.drain([skin])), []);
  assert.deepEqual(plain(host.drain([mod])), []);
});

test("a drain without ids takes everything, oldest first", () => {
  const { host, workers } = page();
  const a = host.spawn();
  const b = host.spawn();
  workers[1].send("b1");
  workers[0].send("a1");
  assert.deepEqual(plain(host.drain()), [
    [b, "msg", "b1"],
    [a, "msg", "a1"],
  ]);
  assert.deepEqual(plain(host.drain()), []);
});

test("terminate drops the worker's undrained messages and further posts fail", () => {
  const { host, workers } = page();
  const a = host.spawn();
  const b = host.spawn();
  workers[0].send("a1");
  workers[1].send("b1");
  host.terminate(a);
  assert.equal(workers[0].terminated, true);
  assert.equal(host.post(a, "x"), false);
  assert.equal(host.post(b, "y"), true);
  assert.deepEqual(workers[1].posted, ["y"]);
  assert.deepEqual(plain(host.drain([a, b])), [[b, "msg", "b1"]]);
});
