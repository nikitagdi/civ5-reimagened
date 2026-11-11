// Tests of the Node skeleton: server annotation, network guard, scan IR, determinism, stdio protocol.
// Run: node --test tools/ui-convert/node/test/
import { test, before, after } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import http from "node:http";
import { spawn } from "node:child_process";
import readline from "node:readline";
import { fileURLToPath } from "node:url";
import { createSession } from "../session.mjs";
import { startServer, annotateHtml } from "../server.mjs";
import { canonicalJson } from "../canonical.mjs";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const FIXTURE = path.join(HERE, "fixtures", "strip");
const MAIN = path.join(HERE, "..", "main.mjs");
// A font that already lives in the repo; the stub serves it under the fixture's font path.
const STUB_FONT = path.join(HERE, "..", "..", "..", "asset-pipeline", "fonts", "Ubuntu-Light.ttf");

const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "ui2-scan-"));
after(() => fs.rmSync(tmp, { recursive: true, force: true }));

const fontsDir = path.join(tmp, "fonts-stub");
fs.mkdirSync(path.join(fontsDir, "fonts"), { recursive: true });
fs.copyFileSync(STUB_FONT, path.join(fontsDir, "fonts", "Stub.ttf"));

/** GET without any URL normalization (fetch would resolve dot segments before sending). */
function rawGet(port, rawPath, headers = {}) {
  return new Promise((resolve, reject) => {
    const req = http.request({ host: "127.0.0.1", port, path: rawPath, method: "GET", headers }, (res) => {
      res.resume();
      res.on("end", () => resolve({ status: res.statusCode }));
    });
    req.on("error", reject);
    req.end();
  });
}

function copyDir(from, to) {
  fs.cpSync(from, to, { recursive: true });
  return to;
}

async function scanDir(dir, { state = "" } = {}) {
  const s = createSession();
  try {
    await s.open({ skin_dir: dir, fonts_dir: fontsDir });
    return await s.scan({ state });
  } finally {
    await s.close();
  }
}

function* walk(n) {
  yield n;
  for (const c of n.children || []) yield* walk(c);
}
const attrOf = (el, name) => (el.attrs.find((a) => a[0] === name) || [])[1];
const byClass = (ir, cls) => [...walk(ir.dom)].filter((n) => n.type === "element" && (attrOf(n, "class") || "").split(/\s+/).includes(cls));

/** 1-based line and column of the first occurrence of `needle` in `text`. */
function lineCol(text, needle) {
  const off = text.indexOf(needle);
  assert.ok(off >= 0, `fixture lacks ${needle}`);
  const before = text.slice(0, off).split("\n");
  return `${before.length}:${before[before.length - 1].length + 1}`;
}

let ir;
before(async () => { ir = await scanDir(FIXTURE, { state: "hud" }); });

test("annotateHtml: adds data-ui2-src and changes nothing else", () => {
  const src = fs.readFileSync(path.join(FIXTURE, "index.html"), "utf8");
  const { html, styles } = annotateHtml(src, "index.html");
  assert.match(html, /<header data-ui2-src="index\.html:14:5" class="strip"/);
  assert.equal(html.replace(/ data-ui2-src="[^"]*"/g, ""), src);
  assert.equal(styles.length, 1);
  assert.equal(styles[0].src, "index.html:8:3");
  assert.match(styles[0].text, /\.hint \{/);
});

test("server: serves files, annotates html, refuses traversal and other methods", async () => {
  const server = await startServer({ root: FIXTURE });
  try {
    const page = await fetch(`${server.url}/index.html`);
    assert.equal(page.status, 200);
    assert.match(page.headers.get("content-type"), /^text\/html/);
    assert.match(await page.text(), /data-ui2-src="index\.html:12:1"/);
    const css = await fetch(`${server.url}/css/strip.css`);
    assert.match(css.headers.get("content-type"), /^text\/css/);
    assert.equal((await fetch(`${server.url}/missing.png`)).status, 404);
    // fetch() normalizes dot segments itself, so the raw request goes through http.request.
    // the URL parser resolves this to /package.json, which is not in the skin root
    assert.equal((await rawGet(server.port, "/%2e%2e/package.json")).status, 404);
    assert.equal((await rawGet(server.port, "/css/..%2f..%2fpackage.json")).status, 403);
    assert.equal((await fetch(`${server.url}/index.html`, { method: "POST" })).status, 405);
    assert.equal((await rawGet(server.port, "/index.html", { Host: "evil.example" })).status, 403);
    assert.match(server.url, /^http:\/\/127\.0\.0\.1:\d+$/);
  } finally {
    await server.close();
  }
});

test("scan: IR top level and element positions", () => {
  assert.deepEqual(Object.keys(ir).sort(), ["compat", "dom", "entry", "errors", "fonts", "resources", "sheets", "state", "title", "version", "viewport"]);
  assert.equal(ir.version, 1);
  assert.equal(ir.state, "hud");
  assert.equal(ir.entry, "index.html");
  assert.deepEqual(ir.viewport, [1920, 1080]);
  assert.equal(ir.compat, "CSS1Compat");
  assert.equal(ir.title, "Top strip");
  assert.equal(ir.dom.tag, "html");
  assert.deepEqual(ir.errors, []);

  const source = fs.readFileSync(path.join(FIXTURE, "index.html"), "utf8");
  const header = byClass(ir, "strip")[0];
  assert.equal(header.tag, "header");
  assert.equal(header.src, `index.html:${lineCol(source, '<header class="strip"')}`);
  const science = byClass(ir, "cell")[1];
  assert.equal(science.src, `index.html:${lineCol(source, '<div class="cell" data-part="strip.science"')}`);
  assert.equal(attrOf(science, "data-part"), "strip.science");
  // Attributes keep their source order and the internal marker is not among them.
  assert.deepEqual(science.attrs.map((a) => a[0]), ["class", "data-part"]);
  const gold = byClass(ir, "cell")[0];
  assert.equal(attrOf(gold, "data-bind-class-toggle"), "is-low:{{economy.gold < 0}}");
  assert.equal(header.children.filter((c) => c.type === "element").length, 5);
});

test("scan: ids are document-order and unique", () => {
  const ids = [...walk(ir.dom)].map((n) => n.id);
  assert.deepEqual(ids, ids.map((_, i) => i));
});

test("scan: NBSP survives in text nodes", () => {
  const val = byClass(ir, "val")[0];
  const text = val.children.find((c) => c.type === "text");
  assert.equal(text.text, "1 234");
  assert.ok(!text.ws);
  assert.deepEqual(val.children.length, 1);
});

test("scan: anonymous runs are marked (flex item text, anonymous blocks)", () => {
  const science = byClass(ir, "cell")[1];
  const texts = science.children.filter((c) => c.type === "text");
  assert.deepEqual(texts.map((t) => t.text), ["+8"]);
  assert.deepEqual(texts[0].anon, { kind: "flex", run: 0 });
  // Whitespace between flex items is not an anonymous item.
  const gold = byClass(ir, "cell")[0];
  assert.ok(gold.children.filter((c) => c.type === "text").every((t) => t.ws && !t.anon));

  const hint = byClass(ir, "hint")[0];
  const kinds = hint.children.map((c) => [c.type === "text" ? JSON.stringify(c.text) : c.tag, c.anon && `${c.anon.kind}${c.anon.run}`]);
  assert.deepEqual(kinds, [['"Intro "', "block0"], ["b", "block0"], ["div", undefined], ['"tail"', "block1"]]);
  // Text inside a plain inline formatting context (the .lbl span) is not anonymous.
  assert.ok(!byClass(ir, "lbl")[0].children[0].anon);
});

test("scan: svg elements keep their case and namespace", () => {
  const svg = [...walk(ir.dom)].find((n) => n.type === "element" && n.tag === "svg");
  assert.equal(svg.ns, "svg");
  assert.equal(attrOf(svg, "viewBox"), "0 0 12 12");
  assert.equal(svg.children.find((c) => c.type === "element").tag, "path");
});

test("scan: stylesheets with css-tree positions", () => {
  assert.deepEqual(ir.sheets.map((s) => [s.kind, s.href || s.src]), [["link", "css/tokens.css"], ["link", "css/strip.css"], ["style", "index.html:8:3"]]);
  const tokens = ir.sheets[0];
  assert.equal(tokens.rules[0].selector, ":root");
  assert.deepEqual(tokens.rules[0].items[0], { type: "decl", prop: "--ink", value: "#ebe2d2", important: false, line: 2, col: 3 });
  const strip = ir.sheets[1];
  const face = strip.rules[0];
  assert.equal(face.type, "atrule");
  assert.equal(face.name, "font-face");
  assert.equal(face.line, 1);
  const hover = strip.rules.find((r) => r.selector === ".menu:hover");
  assert.equal(hover.items[0].prop, "background-color");
  assert.equal(hover.items[0].value, "var(--accent)");
  const css = fs.readFileSync(path.join(FIXTURE, "css", "strip.css"), "utf8");
  assert.equal(`${hover.line}:${hover.col}`, lineCol(css, ".menu:hover"));
  const menu = strip.rules.find((r) => r.selector === ".menu");
  assert.ok(menu.items.some((d) => d.prop === "transition" && d.value === "background-color 120ms ease"));
  // inline <style>: positions are file positions, not block-relative
  const inline = ir.sheets[2];
  const html = fs.readFileSync(path.join(FIXTURE, "index.html"), "utf8");
  assert.equal(`${inline.rules[0].line}:${inline.rules[0].col}`, lineCol(html, ".hint {"));
  assert.deepEqual(strip.parseErrors, []);
});

test("scan: resources and fonts come from the local server, font stub is served", () => {
  const paths = ir.resources.map((r) => r.path);
  assert.deepEqual(paths, [...paths].sort());
  assert.ok(paths.includes("/index.html") && paths.includes("/css/strip.css") && paths.includes("/fonts/Stub.ttf"));
  assert.ok(ir.resources.every((r) => r.status === 200));
  assert.deepEqual(ir.fonts, [{ family: "Stub Sans", style: "normal", weight: "400", status: "loaded" }]);
});

test("scan: byte-identical twice, and for a copy in another directory", async () => {
  const again = await scanDir(FIXTURE, { state: "hud" });
  assert.equal(canonicalJson(again), canonicalJson(ir));
  const copy = copyDir(FIXTURE, path.join(tmp, "elsewhere", "other-name"));
  assert.equal(canonicalJson(await scanDir(copy, { state: "hud" })), canonicalJson(ir));
  // No machine-specific text leaks into the IR.
  assert.ok(!canonicalJson(ir).includes(FIXTURE.replace(/\\/g, "\\\\")));
  assert.doesNotMatch(canonicalJson(ir), /127\.0\.0\.1/);
});

test("network: a fonts CDN link is blocked and reported as E9-001 with the URL", async () => {
  const dir = copyDir(FIXTURE, path.join(tmp, "cdn"));
  const idx = path.join(dir, "index.html");
  const url = "https://fonts.googleapis.com/css2?family=Fira+Sans:wght@400;600&display=swap";
  fs.writeFileSync(idx, fs.readFileSync(idx, "utf8").replace("</head>", `  <link rel="stylesheet" href="${url}">\n</head>`));
  const bad = await scanDir(dir);
  assert.deepEqual(bad.errors, [{ code: "E9-001", message: "request to an address outside the local server was blocked", url }]);
  // The CDN stylesheet is listed as an external sheet, never fetched.
  assert.ok(bad.sheets.some((s) => s.external && s.href === url));
  assert.ok(bad.resources.every((r) => r.path.startsWith("/")));
});

test("network: other loopback ports and script fetches are blocked too", async () => {
  const dir = copyDir(FIXTURE, path.join(tmp, "script"));
  const idx = path.join(dir, "index.html");
  fs.writeFileSync(idx, fs.readFileSync(idx, "utf8").replace("</body>", `<img src="http://127.0.0.1:9/x.png"><script>fetch("https://example.com/data.json").catch(function () {});</script></body>`));
  const bad = await scanDir(dir);
  assert.deepEqual(bad.errors.map((e) => [e.code, e.url]), [["E9-001", "http://127.0.0.1:9/x.png"], ["E9-001", "https://example.com/data.json"]]);
});

test("session: errors for a missing skin, a missing entry, double open, scan before open", async () => {
  const s = createSession();
  await assert.rejects(s.scan({}), { code: "E9-003" });
  await assert.rejects(s.open({ skin_dir: path.join(tmp, "nope") }), { code: "E9-004" });
  await assert.rejects(s.open({ skin_dir: FIXTURE, entry: "nope.html" }), { code: "E9-004" });
  await assert.rejects(s.open({}), { code: "E9-003" });
  await s.open({ skin_dir: FIXTURE });
  await assert.rejects(s.open({ skin_dir: FIXTURE }), { code: "E9-003" });
  await s.close();
});

/** Runs main.mjs and exchanges JSON lines. */
function startMain() {
  const child = spawn(process.execPath, [MAIN], { stdio: ["pipe", "pipe", "inherit"] });
  const rl = readline.createInterface({ input: child.stdout });
  const waiting = [];
  const lines = [];
  rl.on("line", (l) => { lines.push(l); waiting.shift()?.(); });
  const exited = new Promise((r) => child.on("exit", (code) => r(code)));
  return {
    raw: lines,
    send(obj) { child.stdin.write((typeof obj === "string" ? obj : JSON.stringify(obj)) + "\n"); },
    async next() {
      if (!lines.length) await new Promise((r) => waiting.push(r));
      return lines.shift();
    },
    exited,
    child,
  };
}

test("stdio protocol: open, scan, errors, close", async () => {
  const m = startMain();
  m.send({ id: 1, cmd: "open", skin_dir: FIXTURE, fonts_dir: fontsDir });
  const open = JSON.parse(await m.next());
  assert.equal(open.id, 1);
  assert.equal(open.ok, true);
  assert.match(open.result.chrome, /^\d+\.\d+\.\d+\.\d+$/);
  assert.deepEqual(open.result.flags, ["--font-render-hinting=none", "--enable-font-subpixel-positioning", "--disable-lcd-text"]);
  assert.match(open.result.base_url, /^http:\/\/127\.0\.0\.1:\d+\/$/);

  m.send({ id: "s", cmd: "scan", state: "hud" });
  const scanLine = await m.next();
  const scan = JSON.parse(scanLine);
  assert.equal(scan.id, "s");
  assert.equal(scan.ok, true);
  assert.equal(canonicalJson(scan.result), canonicalJson(ir));
  assert.equal(scanLine, canonicalJson({ id: "s", ok: true, result: ir }), "responses are canonical JSON");

  m.send("this is not json");
  const bad = JSON.parse(await m.next());
  assert.deepEqual([bad.id, bad.ok, bad.error.code], [null, false, "E9-003"]);
  m.send({ id: 3, cmd: "shoot" });
  const planned = JSON.parse(await m.next());
  assert.deepEqual([planned.id, planned.ok, planned.error.code], [3, false, "E9-003"]);
  assert.match(planned.error.message, /not implemented/);
  // raster: an SVG at scale 1 and 2, straight RGBA, top half white and opaque, bottom half transparent.
  const svg = (s) => `<svg xmlns="http://www.w3.org/2000/svg" width="${4 * s}" height="${4 * s}" viewBox="0 0 4 4"><rect width="4" height="2" fill="#fff"/></svg>`;
  m.send({ id: 31, cmd: "raster", items: [{ svg1: svg(1), svg2: svg(2), w1: 4, h1: 4, w2: 8, h2: 8 }] });
  const raster = JSON.parse(await m.next());
  assert.equal(raster.ok, true, JSON.stringify(raster.error));
  const x1 = Buffer.from(raster.result.items[0].x1, "base64");
  const x2 = Buffer.from(raster.result.items[0].x2, "base64");
  assert.deepEqual([x1.length, x2.length], [4 * 4 * 4, 8 * 8 * 4]);
  assert.deepEqual([...x1.subarray(0, 4)], [255, 255, 255, 255]);
  assert.deepEqual([...x1.subarray(x1.length - 4)], [0, 0, 0, 0]);
  m.send({ id: 32, cmd: "raster", items: [{ w1: 4 }] });
  assert.equal(JSON.parse(await m.next()).error.code, "E9-003");
  m.send({ id: 4, cmd: "frobnicate" });
  assert.match(JSON.parse(await m.next()).error.message, /unknown command/);
  m.send({ id: 5, cmd: "reload" });
  assert.deepEqual(JSON.parse(await m.next()), { id: 5, ok: true, result: {} });
  m.send({ id: 6, cmd: "close" });
  assert.deepEqual(JSON.parse(await m.next()), { id: 6, ok: true, result: {} });
  assert.equal(await m.exited, 0);
});

test("stdio protocol: closing stdin shuts the process down", async () => {
  const m = startMain();
  m.send({ id: 1, cmd: "open", skin_dir: FIXTURE });
  assert.equal(JSON.parse(await m.next()).ok, true);
  m.child.stdin.end();
  assert.equal(await m.exited, 0);
});
