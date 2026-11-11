// The rectangle dump of Chrome: the boxes of the page as Chrome laid them out, in the one rectangle dump format
// that the runtime writes too (`Runtime::dump_rects`):
//
//   {"v":1, "skin":"engraved", "state":"hud", "viewport":[1920,1080], "dsf":1,
//    "rects":[["0:12","",0,0,1920,48], ["4:0","0:2",1592,64,328,60], ["0:14#0","",20.5,14,61.25,20]]}
//
// A record is the node `<tpl>:<idx>` (the preview's numbering, data-ui2-node), `#k` for the k-th text leaf of
// an element, the instance keys (data-ui2-inst), and x y w h in CSS px rounded to 3 decimals: the bounding box
// with transforms (getBoundingClientRect; a text leaf: a Range over its text without the collapsible white
// space at its ends). Hidden nodes (no box: display none, a false `if`) are not written; boxes clipped by an
// overflow are. Sorted by (node, keys).
//
// Text leaves follow the converter's rule (src/styles.rs, children): a text node is a leaf unless it is only
// collapsible white space and either its element is a flex or grid container or it is not in an anonymous
// block box, and its element holds no other text. An element bound with data-bind-value or data-l10n has one
// leaf, its text.
//
// The page is the preview's `layout` mode: the honest sample of the state, no skin JS.
//
//   rects {state, viewport:[w,h], dsf}  -> one dump
//
// CLI (from the repository root, after `cargo xtask ui build <id>`, which writes the font subsets):
//   node tools/ui-convert/node/rects.mjs skins/engraved --state hud [--viewport 1920x1080] [--dsf 1.5]
//        [--fonts-dir target/ui2/engraved/fonts] [--extended] [--out file]
// Without --viewport the reference viewports are dumped (1280x720, 1920x1200, 3440x1440, 5120x1440; with
// --extended also 1280x720 at dsf 1.5) and the output is a list of dumps, one record per line.
import fs from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";
import { ProtocolError, createSession } from "./session.mjs";
import { entryUrl, loadPage } from "./ir.mjs";

export const REFERENCE = [[1280, 720], [1920, 1200], [3440, 1440], [5120, 1440]];
export const EXTENDED = [[1280, 720, 1.5]];

/** Runs inside the page. Self-contained. */
function dumpInPage() {
  const isWs = (s) => !/[^ \t\n\r\f]/.test(s);
  const r3 = (v) => Math.round(v * 1000) / 1000;
  const INLINE = new Set(["inline", "inline-block", "inline-flex", "inline-grid", "inline-table"]);
  const outOfFlow = (cs) => cs.position === "absolute" || cs.position === "fixed";
  const inlineLevel = (k) => {
    if (k.nodeType === 3) return !isWs(k.data);
    const cs = getComputedStyle(k);
    return INLINE.has(cs.display) && !outOfFlow(cs);
  };
  const blockLevel = (k) => {
    if (k.nodeType !== 1) return false;
    const cs = getComputedStyle(k);
    return !INLINE.has(cs.display) && cs.display !== "none" && cs.display !== "contents" && !outOfFlow(cs) && cs.float === "none";
  };
  // The white space text nodes that sit in an anonymous block run (as the scan marks them).
  const anonBlockText = (el, kids) => {
    const set = new Set();
    const d = getComputedStyle(el).display;
    if (!(d === "block" || d === "list-item" || d === "flow-root" || d === "inline-block")) return set;
    if (!(kids.some(blockLevel) && kids.some(inlineLevel))) return set;
    let cur = [];
    const flush = () => { if (cur.some(inlineLevel)) for (const n of cur) if (n.nodeType === 3) set.add(n); cur = []; };
    for (const k of kids) { if (blockLevel(k)) flush(); else cur.push(k); }
    flush();
    return set;
  };
  // A single-line text cut with `text-overflow: ellipsis`: a Range still spans the whole text, so the record is
  // the text that stays plus the ellipsis, the way the line is drawn. The cut is the longest prefix (whole
  // characters) that fits with the ellipsis into the content box; both widths are Chrome's own measurements.
  const ellipsized = (el, t, start, end, r) => {
    const cs = getComputedStyle(el);
    if (cs.textOverflow !== "ellipsis" || cs.overflowX === "visible") return r;
    const pl = parseFloat(cs.paddingLeft) || 0;
    const pr = parseFloat(cs.paddingRight) || 0;
    const bl = parseFloat(cs.borderLeftWidth) || 0;
    const br = parseFloat(cs.borderRightWidth) || 0;
    const box = el.getBoundingClientRect();
    // The content box edges from the fractional border box (clientLeft/clientWidth are whole pixels; a skin
    // hides scroll bars, so none takes space).
    const right = box.right - br - pr;
    // Boxes are laid out in layout units (1/64 px) while a Range measures the glyph advances in floats: a box
    // sized to its text (a flex item at its max-content width) can be up to 1/64 px narrower than the Range,
    // and Chrome draws that line uncut.
    if (r.right <= right + 1 / 64 + 0.001) return r;
    const probe = document.createElement("span");
    probe.textContent = "…";
    probe.style.cssText = `position:absolute;left:0;top:0;white-space:pre;font:${cs.font};letter-spacing:${cs.letterSpacing};font-variant-numeric:${cs.fontVariantNumeric}`;
    document.body.appendChild(probe);
    const ell = probe.getBoundingClientRect().width;
    probe.remove();
    const avail = right - Math.max(r.left, box.left + bl + pl);
    const range = document.createRange();
    range.setStart(t, start);
    let fit = 0;
    for (let i = start; i < end; ) {
      const cp = t.data.codePointAt(i);
      const next = i + (cp > 0xffff ? 2 : 1);
      range.setEnd(t, next);
      const w = range.getBoundingClientRect().width;
      if (w + ell > avail + 0.001) break;
      fit = w;
      i = next;
    }
    return { x: r.x, y: r.y, width: fit + ell, height: r.height };
  };
  const rects = [];
  const push = (node, keys, r) => rects.push([node, keys, r3(r.x), r3(r.y), r3(r.width), r3(r.height)]);
  for (const el of document.querySelectorAll("[data-ui2-node]")) {
    if (el.getClientRects().length === 0) continue;
    const node = el.getAttribute("data-ui2-node");
    const keys = el.getAttribute("data-ui2-inst") || "";
    push(node, keys, el.getBoundingClientRect());
    if (el.localName === "svg") continue;
    const kids = [...el.childNodes].filter((n) => n.nodeType === 1 || n.nodeType === 3);
    const texts = kids.filter((n) => n.nodeType === 3);
    const d = getComputedStyle(el).display;
    const container = /flex|grid/.test(d);
    const paragraph = texts.some((t) => !isWs(t.data));
    const anon = anonBlockText(el, kids);
    let k = 0;
    for (const t of texts) {
      if (isWs(t.data) && (container || !anon.has(t)) && !paragraph) continue;
      const s = t.data;
      let start = s.search(/[^ \t\n\r\f]/);
      let end = s.length;
      if (start < 0) start = 0;
      else while (end > start && /[ \t\n\r\f]/.test(s[end - 1])) end--;
      const range = document.createRange();
      range.setStart(t, start);
      range.setEnd(t, end);
      push(`${node}#${k}`, keys, ellipsized(el, t, start, end, range.getBoundingClientRect()));
      k++;
    }
  }
  rects.sort((a, b) => (a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : a[1] < b[1] ? -1 : a[1] > b[1] ? 1 : 0));
  return rects;
}

function check(req) {
  const need = (c, m) => { if (!c) throw new ProtocolError("E9-003", `rects: ${m}`); };
  need(typeof req.state === "string" && req.state !== "", "state is a sample name");
  need(Array.isArray(req.viewport) && req.viewport.length === 2 && req.viewport.every((n) => Number.isInteger(n) && n > 0), "viewport is [w, h]");
  need(req.dsf === undefined || (typeof req.dsf === "number" && req.dsf > 0 && req.dsf <= 4), "dsf is a device scale");
}

/**
 * One dump. `viewport` is in CSS px; a dsf other than 1 runs in a context of its own (the device scale of a
 * context is fixed).
 */
export async function rectsOf({ session, req, skin }) {
  check(req);
  const dsf = req.dsf ?? 1;
  const [w, h] = req.viewport;
  const ctx = await session.newContext({ width: w, height: h }, dsf);
  try {
    const page = await ctx.newPage();
    await loadPage(page, entryUrl(session.server(), session.entry(), "layout", req.state));
    // Transitions do not run in a dump.
    await page.addStyleTag({ content: "*{transition:none!important}" });
    const faces = await page.evaluate(() => [...document.fonts].map((f) => `${f.family} ${f.weight} ${f.status}`));
    const bad = faces.filter((f) => !f.endsWith("loaded"));
    if (bad.length) throw new ProtocolError("E9-003", `rects: font faces did not load: ${bad.join(", ")}`);
    const rects = await page.evaluate(dumpInPage);
    return { v: 1, skin, state: req.state, viewport: [w, h], dsf, rects };
  } finally {
    await ctx.close();
  }
}

/** A dump list as text: one record per line, so a diff of a fixture shows the boxes that moved. */
export function formatDumps(dumps) {
  const one = (d) => [
    `{"v": ${d.v}, "skin": ${JSON.stringify(d.skin)}, "state": ${JSON.stringify(d.state)}, "viewport": ${JSON.stringify(d.viewport)}, "dsf": ${d.dsf}, "rects": [`,
    d.rects.map((r) => JSON.stringify(r)).join(",\n"),
    "]}",
  ].join("\n");
  return "[\n" + dumps.map(one).join(",\n") + "\n]\n";
}

async function cli(argv) {
  const opts = { state: "hud", viewport: null, dsf: 1, fontsDir: null, extended: false, out: null };
  const pos = [];
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === "--state") opts.state = argv[++i];
    else if (a === "--viewport") opts.viewport = argv[++i].split("x").map(Number);
    else if (a === "--dsf") opts.dsf = Number(argv[++i]);
    else if (a === "--fonts-dir") opts.fontsDir = argv[++i];
    else if (a === "--extended") opts.extended = true;
    else if (a === "--out") opts.out = argv[++i];
    else pos.push(a);
  }
  if (pos.length !== 1) {
    console.error("usage: node rects.mjs <skin_dir> [--state hud] [--viewport WxH] [--dsf k] [--fonts-dir dir] [--extended] [--out file]");
    return 2;
  }
  const skinDir = pos[0];
  const toml = fs.readFileSync(path.join(skinDir, "skin.toml"), "utf8");
  const id = (/^\s*id\s*=\s*"([^"]+)"/m.exec(toml) || [])[1] || path.basename(path.resolve(skinDir));
  const fontsDir = opts.fontsDir ?? path.join("target", "ui2", id, "fonts");
  if (!fs.existsSync(fontsDir)) {
    console.error(`error: no font subsets in ${fontsDir}; run \`cargo xtask ui build ${id}\` first or pass --fonts-dir`);
    return 1;
  }
  const session = createSession();
  try {
    await session.open({ skin_dir: skinDir, fonts_dir: fontsDir, preview: true });
    const list = opts.viewport ? [[...opts.viewport, opts.dsf]] : REFERENCE.map(([w, h]) => [w, h, 1]).concat(opts.extended ? EXTENDED : []);
    const dumps = [];
    for (const [w, h, dsf] of list) dumps.push(await session.rects({ state: opts.state, viewport: [w, h], dsf }));
    const text = opts.viewport ? JSON.stringify(dumps[0]) + "\n" : formatDumps(dumps);
    if (opts.out) fs.writeFileSync(opts.out, text);
    else process.stdout.write(text);
    return 0;
  } catch (e) {
    console.error(`error[${e.code || "E9-002"}]: ${e.message}`);
    return 1;
  } finally {
    await session.close();
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  process.exitCode = await cli(process.argv.slice(2));
}
