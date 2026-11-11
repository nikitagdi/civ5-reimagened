// TEMPORARY fixture generator for the top strip layout test (tests/layout_top.rs).
//
// Replaced by the converter's rectangle dump (tools/ui-convert/node/rects.mjs) and its top strip package
// fixture once those exist; delete this script and tests/fixtures/top-strip-chrome.json then.
//
// What it does: serves skins/engraved with the converter's local server, opens index.html in the pinned
// Playwright Chromium, expands the `data-include` placeholders in the page, loads every font face, turns
// transitions off, and for each reference viewport dumps
//   - the rectangle of every element of the HUD layer root and the top strip (getBoundingClientRect; SVG
//     internals excluded) and of every text node (a Range over the text without collapsible white space),
//     in the rectangle dump format: [node, keys, x, y, w, h] in CSS px with 3 decimals;
//   - once, the structure (tag, classes, part, text) and the computed layout and font styles of each node
//     (Typed OM `computedStyleMap()`, `getComputedStyle` for border widths, as the Typed OM spike says).
// Node names follow the dump format: `0:<i>` is the i-th element in preorder (the HUD layer root is 0, the
// top strip header is 1), `0:<i>#<k>` the k-th non-white-space text node of element i.
//
// Usage (from the repository root, once `npm ci` was run in tools/playtest and tools/ui-convert/node):
//   node crates/client/ui2-runtime/tests/support/top_strip_chrome.mjs
import fs from "node:fs";
import { fileURLToPath } from "node:url";

const ROOT = new URL("../../../../../", import.meta.url);
const { launchChromium, chromiumFlags } = await import(new URL("tools/ui-convert/node/chromium.mjs", ROOT).href);
const { startServer } = await import(new URL("tools/ui-convert/node/server.mjs", ROOT).href);

const SKIN = fileURLToPath(new URL("skins/engraved/", ROOT));
const OUT = new URL("../fixtures/top-strip-chrome.json", import.meta.url);
// Reference viewports of the layout check (CSS px at device scale 1).
const VIEWPORTS = [[1280, 720], [1920, 1200], [3440, 1440], [5120, 1440]];

// `[fonts.files]` of skin.toml: the path a stylesheet uses -> the font file in the repository.
function fontFiles() {
  const toml = fs.readFileSync(new URL("skins/engraved/skin.toml", ROOT), "utf8");
  const section = toml.split(/^\[fonts\.files\]\s*$/m)[1]?.split(/^\[/m)[0] ?? "";
  const map = new Map();
  for (const m of section.matchAll(/^"([^"]+)"\s*=\s*"([^"]+)"/gm)) map.set(m[1], m[2]);
  return map;
}

const TYPED = [
  "display", "position", "box-sizing", "top", "right", "bottom", "left", "width", "height", "min-width",
  "min-height", "max-width", "max-height", "margin-top", "margin-right", "margin-bottom", "margin-left",
  "padding-top", "padding-right", "padding-bottom", "padding-left", "overflow-x", "overflow-y",
  "flex-direction", "flex-wrap", "flex-grow", "flex-shrink", "flex-basis", "order", "justify-content",
  "align-items", "align-self", "align-content", "row-gap", "column-gap", "font-family", "font-size",
  "font-weight", "font-style", "font-variant-numeric", "line-height", "letter-spacing", "word-spacing",
  "text-align", "white-space-collapse", "text-wrap-mode", "text-overflow",
];
const RESOLVED = ["border-top-width", "border-right-width", "border-bottom-width", "border-left-width"];

function extract({ typed, resolved }) {
  const hud = document.querySelector('.layer[data-layer="hud"]');
  const top = hud.querySelector("header.top");
  const elements = [hud];
  const walk = (el) => {
    elements.push(el);
    if (el.localName === "svg") return;
    for (const c of el.children) walk(c);
  };
  walk(top);
  const index = new Map(elements.map((el, i) => [el, i]));
  const r3 = (v) => Math.round(v * 1000) / 1000;
  const box = (r) => [r3(r.x), r3(r.y), r3(r.width), r3(r.height)];
  const ws = /^[ \t\n\r\f]*$/;
  const nodes = [];
  const rects = [];
  // Nodes in document order: an element, then its children (elements and text nodes interleaved).
  const visit = (el) => {
    const i = index.get(el);
    const parent = el === hud ? null : `0:${index.get(el.parentElement)}`;
    const cs = getComputedStyle(el);
    const map = el.computedStyleMap();
    const style = {};
    for (const p of typed) style[p] = String(map.get(p));
    for (const p of resolved) style[p] = cs.getPropertyValue(p);
    nodes.push({
      node: `0:${i}`, parent, tag: el.localName, classes: [...el.classList].join(" "),
      part: el.getAttribute("data-part") || "", text: null, style,
    });
    rects.push([`0:${i}`, "", ...box(el.getBoundingClientRect())]);
    if (el.localName === "svg") return;
    let k = 0;
    // The HUD layer root also holds the icon sprite: only the strip is visited below it.
    for (const t of el === hud ? [top] : el.childNodes) {
      if (index.has(t)) {
        visit(t);
        continue;
      }
      if (t.nodeType !== Node.TEXT_NODE || ws.test(t.data)) continue;
      const s = t.data;
      const start = s.search(/[^ \t\n\r\f]/);
      let end = s.length;
      while (end > start && /[ \t\n\r\f]/.test(s[end - 1])) end--;
      const range = document.createRange();
      range.setStart(t, start);
      range.setEnd(t, end);
      const text = s.slice(start, end).replace(/[ \t\n\r\f]+/g, " ");
      nodes.push({ node: `0:${i}#${k}`, parent: `0:${i}`, tag: "#text", classes: "", part: "", text, style: null });
      rects.push([`0:${i}#${k}`, "", ...box(range.getBoundingClientRect())]);
      k++;
    }
  };
  visit(hud);
  return { nodes, rects };
}

const fonts = fontFiles();
const server = await startServer({ root: SKIN });
const browser = await launchChromium();
try {
  const context = await browser.newContext({ viewport: { width: 1920, height: 1080 }, deviceScaleFactor: 1 });
  await context.route("**/*", async (route) => {
    const u = new URL(route.request().url());
    if (u.origin !== server.url) return route.abort("blockedbyclient");
    const file = fonts.get(decodeURIComponent(u.pathname.slice(1)));
    if (file) return route.fulfill({ status: 200, contentType: "font/ttf", body: fs.readFileSync(new URL(file, ROOT)) });
    return route.continue();
  });
  const page = await context.newPage();
  await page.goto(`${server.url}/index.html`, { waitUntil: "load" });
  const faces = await page.evaluate(async () => {
    for (const el of [...document.querySelectorAll("[data-include]")]) {
      const html = await (await fetch(el.getAttribute("data-include"))).text();
      const t = document.createElement("template");
      t.innerHTML = html;
      el.replaceWith(t.content);
    }
    const kill = document.createElement("style");
    kill.textContent = "*,*::before,*::after{transition:none!important;animation:none!important}";
    document.head.append(kill);
    await Promise.all([...document.fonts].map((f) => f.load().catch(() => null)));
    await document.fonts.ready;
    return [...document.fonts].map((f) => `${f.family} ${f.weight} ${f.status}`);
  });
  if (faces.some((f) => !f.endsWith("loaded"))) throw new Error(`fonts did not load: ${faces.join(", ")}`);

  let structure = null;
  const viewports = [];
  for (const [w, h] of VIEWPORTS) {
    await page.setViewportSize({ width: w, height: h });
    const d = await page.evaluate(extract, { typed: TYPED, resolved: RESOLVED });
    const s = JSON.stringify(d.nodes);
    if (structure === null) structure = s;
    else if (structure !== s) throw new Error(`computed styles differ at ${w}x${h}`);
    d.rects.sort((a, b) => (a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : 0));
    viewports.push({ viewport: [w, h], dsf: 1, rects: d.rects });
  }
  const out = {
    v: 1,
    note: "TEMPORARY fixture from tests/support/top_strip_chrome.mjs; replaced by the converter's top strip fixture",
    skin: "engraved",
    state: "static sample text of parts/top.html",
    // Major version only: a four-part version string reads like an address to the leak scanner.
    chrome: browser.version().split(".")[0],
    flags: chromiumFlags(),
    nodes: JSON.parse(structure),
    viewports,
  };
  // One node or rect per line keeps the diff of a re-run readable.
  const lines = [
    "{",
    `"v": ${out.v}, "note": ${JSON.stringify(out.note)},`,
    `"skin": ${JSON.stringify(out.skin)}, "state": ${JSON.stringify(out.state)},`,
    `"chrome": ${JSON.stringify(out.chrome)}, "flags": ${JSON.stringify(out.flags)},`,
    `"nodes": [`,
    out.nodes.map((n) => JSON.stringify(n)).join(",\n"),
    `],`,
    `"viewports": [`,
    out.viewports
      .map((v) => `{"viewport": ${JSON.stringify(v.viewport)}, "dsf": 1, "rects": [\n${v.rects.map((r) => JSON.stringify(r)).join(",\n")}\n]}`)
      .join(",\n"),
    `]`,
    "}",
  ];
  fs.mkdirSync(new URL(".", OUT), { recursive: true });
  fs.writeFileSync(OUT, lines.join("\n") + "\n");
  console.log(`wrote ${fileURLToPath(OUT).split(/[\\/]/).slice(-2).join("/")}: ${out.nodes.length} nodes, ${VIEWPORTS.length} viewports`);
} finally {
  await browser.close();
  await server.close();
}
