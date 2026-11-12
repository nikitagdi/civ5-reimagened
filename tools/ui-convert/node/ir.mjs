// The page scan: the DOM, the stylesheets and the loaded resources of the page as one deterministic IR.
//
//   IR = { version, state, entry, viewport:[w,h], compat, title, dom, sheets, fonts, resources, errors }
//
//   dom      element tree. Element:  { type:"element", id, tag, ns?, src?, mark?, inst?, tooltip?, attrs:[[name,value]...],
//            natural?:[w,h], anon?, children:[...] }. Text: { type:"text", id, text, ws?, anon? }.
//            ids are assigned in document order over elements and text nodes. Text is exactly what the DOM
//            holds (NBSP stays U+00A0). ws marks text made only of ASCII whitespace (collapsible).
//            anon marks content that forms an anonymous box: { kind:"flex"|"grid"|"block", run:n } where run
//            numbers the anonymous runs of one parent in order (text directly inside a flex/grid container
//            is an anonymous item; inline content between block siblings is an anonymous block).
//            src = "<file>:<line>:<column>" of the start tag in the source (from data-ui2-src).
//   sheets   one entry per <link rel=stylesheet> / <style> in document order, with the css-tree AST reduced
//            to rules / at-rules / declarations and their positions.
//   fonts    document.fonts faces with their load status.
//   resources  [{path, status}] requested from the local server, sorted.
//   errors   [{code, message, url?}] collected while loading (E9-001: request outside the local server).
//
import path from "node:path";
import fs from "node:fs";
import * as csstree from "css-tree";

export const IR_VERSION = 1;

/** Runs inside the page. Must be self-contained (it is serialized). */
function extractInPage() {
  let nextId = 0;
  const INTERNAL = new Set(["data-ui2-src", "data-ui2-node", "data-ui2-inst", "data-ui2-asset", "data-ui2-tooltip"]);
  const SVG_NS = "http://www.w3.org/2000/svg";
  const isWs = (s) => !/[^ \t\n\r\f]/.test(s);
  const INLINE_DISPLAY = new Set(["inline", "inline-block", "inline-flex", "inline-grid", "inline-table"]);
  const outOfFlow = (cs) => cs.position === "absolute" || cs.position === "fixed";

  const isInlineLevel = (k) => {
    if (k.nodeType === 3) return !isWs(k.data);
    const cs = getComputedStyle(k);
    return INLINE_DISPLAY.has(cs.display) && !outOfFlow(cs);
  };
  const isBlockLevel = (k) => {
    if (k.nodeType !== 1) return false;
    const cs = getComputedStyle(k);
    return !INLINE_DISPLAY.has(cs.display) && cs.display !== "none" && cs.display !== "contents" && !outOfFlow(cs) && cs.float === "none";
  };

  // Which children of el live in anonymous boxes. Returns Map(node -> {kind, run}).
  function anonMap(el, kids) {
    const map = new Map();
    const display = getComputedStyle(el).display;
    if (/flex|grid/.test(display)) {
      const kind = display.includes("grid") ? "grid" : "flex";
      let run = -1;
      let seq = [];
      const flush = () => {
        if (seq.some((n) => !isWs(n.data))) { run++; for (const n of seq) map.set(n, { kind, run }); }
        seq = [];
      };
      for (const k of kids) { if (k.nodeType === 3) seq.push(k); else flush(); }
      flush();
    } else if (display === "block" || display === "list-item" || display === "flow-root" || display === "inline-block") {
      if (kids.some(isBlockLevel) && kids.some(isInlineLevel)) {
        let run = -1;
        let cur = [];
        const flush = () => {
          if (cur.some(isInlineLevel)) { run++; for (const n of cur) map.set(n, { kind: "block", run }); }
          cur = [];
        };
        for (const k of kids) { if (isBlockLevel(k)) flush(); else cur.push(k); }
        flush();
      }
    }
    return map;
  }

  function textIr(node, anon) {
    const ir = { type: "text", id: nextId++, text: node.data };
    if (isWs(node.data)) ir.ws = true;
    if (anon) ir.anon = anon;
    return ir;
  }

  function elementIr(el, anon) {
    const ir = { type: "element", id: nextId++, tag: el.localName };
    if (el.namespaceURI === SVG_NS) ir.ns = "svg";
    const src = el.getAttribute("data-ui2-src");
    if (src) ir.src = src;
    const mark = el.getAttribute("data-ui2-node");
    if (mark) ir.mark = mark;
    const inst = el.getAttribute("data-ui2-inst");
    if (inst) ir.inst = inst;
    // The root of a tooltip template, drawn in place of its <template> by the preview's extract mode.
    const tooltip = el.getAttribute("data-ui2-tooltip");
    if (tooltip) ir.tooltip = tooltip;
    ir.attrs = [];
    for (const a of el.attributes) if (!INTERNAL.has(a.name)) ir.attrs.push([a.name, a.value]);
    if (el.localName === "img" && el.naturalWidth > 0) ir.natural = [el.naturalWidth, el.naturalHeight];
    if (anon) ir.anon = anon;
    const container = el.localName === "template" && el.namespaceURI !== SVG_NS ? el.content : el;
    const kids = [...container.childNodes].filter((n) => n.nodeType === 1 || n.nodeType === 3);
    const rendered = el.isConnected && el.namespaceURI !== SVG_NS && getComputedStyle(el).display !== "none" && getComputedStyle(el).display !== "contents";
    const map = rendered ? anonMap(el, kids) : new Map();
    ir.children = kids.map((k) => (k.nodeType === 3 ? textIr(k, map.get(k)) : elementIr(k, map.get(k))));
    return ir;
  }

  const dom = elementIr(document.documentElement, null);
  const fonts = [...document.fonts].map((f) => ({
    family: f.family.replace(/^["']|["']$/g, ""),
    style: f.style,
    weight: f.weight,
    status: f.status,
  }));
  return { dom, fonts, title: document.title, compat: document.compatMode };
}

function* walkElements(el) {
  yield el;
  for (const c of el.children) if (c.type === "element") yield* walkElements(c);
}

const attr = (el, name) => (el.attrs.find((a) => a[0] === name) || [])[1];

/** css-tree node -> plain data with positions (1-based line and column of the node start). */
function simplify(node) {
  const pos = node.loc ? { line: node.loc.start.line, col: node.loc.start.column } : {};
  switch (node.type) {
    case "Rule":
      return {
        type: "rule", ...pos,
        selector: csstree.generate(node.prelude),
        selectors: node.prelude.type === "SelectorList" ? node.prelude.children.toArray().map((s) => csstree.generate(s)) : [csstree.generate(node.prelude)],
        items: node.block.children.toArray().map(simplify),
      };
    case "Atrule":
      return {
        type: "atrule", name: node.name, prelude: node.prelude ? csstree.generate(node.prelude) : "", ...pos,
        items: node.block ? node.block.children.toArray().map(simplify) : undefined,
      };
    case "Declaration":
      return { type: "decl", prop: node.property, value: csstree.generate(node.value).trim(), important: !!node.important, ...pos };
    default:
      return { type: node.type.toLowerCase(), ...pos, text: csstree.generate(node) };
  }
}

function parseCss(text, file, line = 1, column = 1) {
  const parseErrors = [];
  const ast = csstree.parse(text, {
    positions: true, filename: file, line, column,
    onParseError: (e) => parseErrors.push({ message: e.message, line: e.line, col: e.column }),
  });
  return { rules: ast.children.toArray().map(simplify), parseErrors };
}

function splitSrc(src) {
  const m = /^(.*):(\d+):(\d+)$/.exec(src || "");
  return m ? { file: m[1], line: Number(m[2]), col: Number(m[3]) } : null;
}

function buildSheets(dom, server) {
  const sheets = [];
  for (const el of walkElements(dom)) {
    if (el.ns) continue;
    if (el.tag === "link" && /(^|\s)stylesheet(\s|$)/i.test(attr(el, "rel") || "")) {
      const href = attr(el, "href") || "";
      const sheet = { kind: "link", node: el.id, src: el.src, href };
      if (/^([a-z][a-z0-9+.-]*:|\/\/)/i.test(href)) {
        sheet.external = true;
      } else {
        const from = splitSrc(el.src);
        const rel = path.posix.normalize(path.posix.join(path.posix.dirname(from ? from.file : "index.html"), href.split(/[?#]/)[0]));
        const text = rel.startsWith("..") ? null : server.readText(rel);
        sheet.file = rel;
        if (text === null) sheet.missing = true;
        else Object.assign(sheet, parseCss(text, rel));
      }
      sheets.push(sheet);
    } else if (el.tag === "style") {
      const from = splitSrc(el.src);
      const block = from && server.styleBlocks(from.file).find((b) => b.src === el.src);
      const sheet = { kind: "style", node: el.id, src: el.src };
      if (block) Object.assign(sheet, parseCss(block.text, from.file, block.line, block.col));
      else Object.assign(sheet, { rules: [], parseErrors: [] });
      sheets.push(sheet);
    }
  }
  return sheets;
}

/**
 * Loads the entry page in `page` (a fresh navigation every time, so a scan never depends on a previous
 * one) and builds the IR. `blocked` is the set of external URLs the network guard refused.
 */
/**
 * The URL of the entry page in a preview mode (`extract` for the converter's scan and style reads, `layout` for
 * the honest sample of the rectangle dump) and sample state. Without the preview script the parameters are
 * ignored by the page.
 */
export function entryUrl(server, entry, mode, state) {
  const q = new URLSearchParams();
  if (mode) q.set("ui2mode", mode);
  if (state) q.set("ui2state", state);
  const s = q.toString();
  return `${server.url}/${entry}${s ? `?${s}` : ""}`;
}

/** Loads the entry page and waits until the preview script (if any) finished and every font face loaded. */
export async function loadPage(page, url) {
  await page.goto(url, { waitUntil: "load" });
  // With the preview script the page is not final before includes and bindings are done.
  await page.evaluate(() => (window.__ui2preview ? window.__ui2preview.ready : null));
  await page.evaluate(async () => {
    await Promise.all([...document.fonts].map((f) => f.load().catch(() => null)));
    await document.fonts.ready;
  });
}

export async function scanPage({ page, server, entry, state = "", viewport = [1920, 1080], blocked, mode = "extract" }) {
  server.reset();
  blocked.clear();
  await page.setViewportSize({ width: viewport[0], height: viewport[1] });
  await loadPage(page, entryUrl(server, entry, mode, state));
  const { dom, fonts, title, compat } = await page.evaluate(extractInPage);

  const fontKey = (f) => `${f.family}|${f.weight}|${f.style}`;
  fonts.sort((a, b) => (fontKey(a) < fontKey(b) ? -1 : fontKey(a) > fontKey(b) ? 1 : 0));
  const errors = [...blocked].sort().map((url) => ({ code: "E9-001", message: "request to an address outside the local server was blocked", url }));
  return {
    version: IR_VERSION,
    state,
    entry,
    viewport,
    compat,
    title,
    dom,
    sheets: buildSheets(dom, server),
    fonts,
    resources: server.requests().filter((r) => r.path !== "/favicon.ico"),
    errors,
  };
}
