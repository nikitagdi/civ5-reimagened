// Tests of the package routes of the local server (watch mode): /pkg/<id>.{ui2,rev,errors.json}.
// Run: node --test tools/ui-convert/node/test/server.test.mjs
import { test, after } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { startServer, pkgType } from "../server.mjs";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const FIXTURE = path.join(HERE, "fixtures", "strip");

const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "ui2-server-"));
after(() => fs.rmSync(tmp, { recursive: true, force: true }));

test("package file names", () => {
  assert.equal(pkgType("engraved.ui2"), "application/octet-stream");
  assert.equal(pkgType("engraved.rev"), "text/plain; charset=utf-8");
  assert.equal(pkgType("engraved.errors.json"), "application/json; charset=utf-8");
  for (const bad of ["../engraved.ui2", "engraved.json", "Engraved.ui2", "a/b.rev", ".rev", "x.ui2.tmp"]) {
    assert.equal(pkgType(bad), null, bad);
  }
});

test("packages are served cross-origin from the package folder only", async () => {
  const pkgDir = path.join(tmp, "pkg");
  fs.mkdirSync(pkgDir, { recursive: true });
  fs.writeFileSync(path.join(pkgDir, "strip.rev"), "3");
  fs.writeFileSync(path.join(pkgDir, "strip.ui2"), Buffer.from([1, 2, 3]));
  fs.writeFileSync(path.join(pkgDir, "strip.errors.json"), "[]\n");
  const server = await startServer({ root: FIXTURE, pkgDir });
  try {
    const rev = await fetch(`${server.url}/pkg/strip.rev`);
    assert.equal(rev.status, 200);
    assert.equal(await rev.text(), "3");
    assert.equal(rev.headers.get("access-control-allow-origin"), "*");
    assert.equal(rev.headers.get("cross-origin-resource-policy"), "cross-origin");
    assert.equal(rev.headers.get("cache-control"), "no-store");
    const pkg = await fetch(`${server.url}/pkg/strip.ui2`);
    assert.deepEqual([...new Uint8Array(await pkg.arrayBuffer())], [1, 2, 3]);
    assert.equal((await fetch(`${server.url}/pkg/strip.errors.json`)).headers.get("content-type"), "application/json; charset=utf-8");
    const missing = await fetch(`${server.url}/pkg/other.ui2`);
    assert.equal(missing.status, 404);
    assert.equal(missing.headers.get("access-control-allow-origin"), "*");
    assert.equal((await fetch(`${server.url}/pkg/strip.toml`)).status, 404);
    const pre = await fetch(`${server.url}/pkg/strip.rev`, { method: "OPTIONS" });
    assert.equal(pre.status, 204);
    assert.equal(pre.headers.get("access-control-allow-origin"), "*");
    // The skin itself is still served, without the cross-origin headers.
    const page = await fetch(`${server.url}/index.html`);
    assert.equal(page.status, 200);
    assert.equal(page.headers.get("access-control-allow-origin"), null);
  } finally {
    await server.close();
  }
});

test("virtual files are served before the folder's own, HTML annotated like a file", async () => {
  const server = await startServer({
    root: FIXTURE,
    virtualFiles: { "css/page.html": "<!doctype html>\n<body>\n<div data-include=\"x.html\"></div>\n</body>\n", "css/extra.css": ".a { color: red; }\n" },
  });
  try {
    const page = await fetch(`${server.url}/css/page.html`);
    assert.equal(page.status, 200);
    assert.match(await page.text(), /<div data-ui2-src="css\/page\.html:3:1" data-include="x\.html">/);
    const css = await fetch(`${server.url}/css/extra.css`);
    assert.equal(css.headers.get("content-type"), "text/css; charset=utf-8");
    assert.equal(await css.text(), ".a { color: red; }\n");
    assert.equal(server.readText("css/extra.css"), ".a { color: red; }\n");
    assert.match(server.readText("css/strip.css"), /\S/, "a file of the folder");
    assert.equal(server.readText("css/none.css"), null);
  } finally {
    await server.close();
  }
});

test("without a package folder there are no package routes", async () => {
  const server = await startServer({ root: FIXTURE });
  try {
    const r = await fetch(`${server.url}/pkg/strip.rev`);
    assert.equal(r.status, 404);
    assert.equal(r.headers.get("access-control-allow-origin"), null);
  } finally {
    await server.close();
  }
});
