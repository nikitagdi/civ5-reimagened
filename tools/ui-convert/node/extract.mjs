// Style extraction: Chromium as the style compiler.
//
// The request is the converter's plan (Rust, src/plan.rs); this file only reads what Chrome
// computed and never interprets a value (numbers stay Chrome's strings).
//
//   extract {cells:[[w,h]...], props:[{name, api:"T"|"C"}...], beacon_props:[i...], extra:[name...],
//            tokens:[name...], nodes:[{id, conds:[{up, kind, class?}...]}...]}
//   -> {beacon, tokens:[{name, value, color}], cells:[{viewport, nodes:[{id, base:[...], extra:[...],
//       off:[[name, value, initial]...], vars:[{mask, set:[[i, value]...]}...],
//       beacon:{base:[[i, value]...], vars:[{mask, set:[[i, value]...]}...]}}]}]}
//
// Node ids are the ids of the scan IR: the page is loaded again and walked in the same order as
// ir.mjs (elements and text nodes in document order, template contents included), so the same
// document gives the same ids however the DOM was expanded (by hand or by the preview script).
//
// Steps per page load: index the nodes; remove `data-ui2-dev` elements (development-only
// stylesheets and elements are not part of the skin); attach the kill-sheet
// `*{transition:none!important}` so forced states and viewport changes apply at once (transition
// properties are read with the kill-sheet off). Per cell: set the viewport; read every node's base
// values, the extra values and the off-profile differences from initial values (an element of the
// same tag in an empty frame); then each non-empty mask of each node's conditions (classes added, `disabled` set,
// pseudo-classes forced over CDP). Then the same reads with the beacon sheet on, which replaces
// every root color token by rgb(R, i >> 8, i & 255); R is the smallest value no base color uses.
import { ProtocolError } from "./session.mjs";

/** Runs inside the page: indexes the nodes as ir.mjs numbers them, and installs helpers. */
function installInPage() {
  const nodes = [];
  const walk = (n) => {
    nodes.push(n);
    if (n.nodeType !== 1) return;
    const container = n.localName === "template" && n.namespaceURI !== "http://www.w3.org/2000/svg" ? n.content : n;
    for (const k of container.childNodes) if (k.nodeType === 1 || k.nodeType === 3) walk(k);
  };
  walk(document.documentElement);
  const ids = new Map(nodes.map((n, i) => [n, i]));
  for (const el of [...document.querySelectorAll("[data-ui2-dev]")]) el.remove();
  const kill = document.createElement("style");
  kill.textContent = "*{transition:none!important}";
  document.head.appendChild(kill);
  const beacon = document.createElement("style");
  document.head.appendChild(beacon);
  beacon.sheet.disabled = true;
  // Initial values: an element of the same tag (and namespace) in an empty frame, so the user
  // agent's defaults of the tag are the baseline and anything the skin or inheritance changed
  // shows. The frame lives only while the baselines are read.
  const frame = document.createElement("iframe");
  document.body.appendChild(frame);
  const fdoc = frame.contentDocument;
  fdoc.open();
  fdoc.write("<!doctype html><html><head></head><body></body></html>");
  fdoc.close();
  const baselines = new Map();
  for (const n of nodes) {
    if (n.nodeType !== 1) continue;
    const key = `${n.namespaceURI} ${n.localName}`;
    if (baselines.has(key)) continue;
    const probe = fdoc.createElementNS(n.namespaceURI, n.localName);
    const host = n.namespaceURI === "http://www.w3.org/2000/svg" && n.localName !== "svg"
      ? fdoc.body.appendChild(fdoc.createElementNS(n.namespaceURI, "svg"))
      : fdoc.body;
    host.appendChild(probe);
    const ics = frame.contentWindow.getComputedStyle(probe);
    const values = {};
    for (let i = 0; i < ics.length; i++) values[ics[i]] = ics.getPropertyValue(ics[i]);
    baselines.set(key, values);
  }
  frame.remove();

  const el = (id) => nodes[id];
  const ancestor = (id, up) => {
    let e = nodes[id];
    for (let k = 0; k < up && e; k++) e = e.parentElement;
    return e;
  };
  const read = (e, props, pick) => {
    const out = [];
    let map = null;
    let cs = null;
    let needKillOff = false;
    for (const i of pick) {
      const p = props[i];
      if (p.name.startsWith("transition-")) { needKillOff = true; out.push(null); continue; }
      if (p.api === "T") {
        map = map || e.computedStyleMap();
        const v = map.get(p.name);
        out.push(v == null ? "" : v.toString());
      } else {
        cs = cs || getComputedStyle(e);
        out.push(cs.getPropertyValue(p.name));
      }
    }
    if (needKillOff) {
      kill.sheet.disabled = true;
      const cs2 = getComputedStyle(e);
      pick.forEach((i, k) => { if (out[k] === null) out[k] = cs2.getPropertyValue(props[i].name); });
      kill.sheet.disabled = false;
    }
    return out;
  };
  const offProfile = (e, known) => {
    const cs = getComputedStyle(e);
    const initial = baselines.get(`${e.namespaceURI} ${e.localName}`) || {};
    const out = [];
    for (let i = 0; i < cs.length; i++) {
      const name = cs[i];
      if (name.startsWith("--") || known.has(name)) continue;
      const v = cs.getPropertyValue(name);
      if (v !== initial[name]) out.push([name, v, initial[name] ?? ""]);
    }
    out.sort((a, b) => (a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : 0));
    return out;
  };
  const setClass = (id, up, cls, on) => {
    const a = ancestor(id, up);
    if (a) a.classList.toggle(cls, on);
  };
  const setDisabled = (id, up, on) => {
    const a = ancestor(id, up);
    if (!a) return;
    if (on) a.setAttribute("disabled", ""); else a.removeAttribute("disabled");
  };
  const tokenValues = (names) => {
    const root = document.documentElement;
    const cs = getComputedStyle(root);
    const probe = document.createElement("div");
    document.body.appendChild(probe);
    const out = names.map((name) => {
      const value = cs.getPropertyValue(name).trim();
      let color = null;
      if (value && CSS.supports("color", value)) {
        probe.style.color = `var(${name})`;
        color = getComputedStyle(probe).color;
      }
      return { name, value, color };
    });
    probe.remove();
    return out;
  };
  const setBeacon = (text) => {
    beacon.textContent = text;
    beacon.sheet.disabled = text === "";
  };
  window.__ui2x = { el, ancestor, read, offProfile, setClass, setDisabled, tokenValues, setBeacon, ids };
  return nodes.length;
}

const PSEUDO = new Set(["hover", "active", "focus", "focus-visible"]);

function checkRequest(req) {
  const need = (c, m) => { if (!c) throw new ProtocolError("E9-003", `extract: ${m}`); };
  need(Array.isArray(req.cells) && req.cells.length > 0 && req.cells.every((c) => Array.isArray(c) && c.length === 2 && c.every((n) => Number.isInteger(n) && n > 0)), "cells is a list of [w, h]");
  need(Array.isArray(req.props) && req.props.every((p) => p && typeof p.name === "string" && (p.api === "T" || p.api === "C")), "props is a list of {name, api}");
  need(Array.isArray(req.beacon_props) && req.beacon_props.every((i) => Number.isInteger(i) && i >= 0 && i < req.props.length), "beacon_props indexes props");
  need(Array.isArray(req.extra) && req.extra.every((s) => typeof s === "string"), "extra is a list of names");
  need(Array.isArray(req.tokens) && req.tokens.every((s) => typeof s === "string" && s.startsWith("--")), "tokens is a list of custom property names");
  need(Array.isArray(req.nodes) && req.nodes.every((n) => Number.isInteger(n.id) && Array.isArray(n.conds) && n.conds.length <= 4
    && n.conds.every((c) => Number.isInteger(c.up) && c.up >= 0 && (PSEUDO.has(c.kind) || c.kind === "disabled" || (c.kind === "class" && typeof c.class === "string")))), "nodes is a list of {id, conds (at most 4)}");
}

/** Waits until style and layout follow a viewport change. */
async function settle(page) {
  await page.evaluate(() => new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(() => r()))));
}

/**
 * Loads the entry page again and answers the plan. `page`, `server`, `entry` as in scanPage.
 */
export async function extractPage({ page, server, entry, req }) {
  checkRequest(req);
  server.reset();
  await page.setViewportSize({ width: req.cells[0][0], height: req.cells[0][1] });
  await page.goto(`${server.url}/${entry}`, { waitUntil: "load" });
  await page.evaluate(() => document.fonts.ready);
  const count = await page.evaluate(installInPage);
  for (const n of req.nodes) if (n.id >= count) throw new ProtocolError("E9-003", `extract: node ${n.id} does not exist in the page`);

  // CDP for forced pseudo-classes: one node id per element that owns a forced state.
  const cdp = await page.context().newCDPSession(page);
  await cdp.send("DOM.enable");
  await cdp.send("CSS.enable");
  await cdp.send("DOM.getDocument", { depth: -1 });
  const cdpIds = new Map();
  const cdpNode = async (id, up) => {
    const key = `${id}/${up}`;
    if (cdpIds.has(key)) return cdpIds.get(key);
    const { result } = await cdp.send("Runtime.evaluate", { expression: `window.__ui2x.ancestor(${id}, ${up})` });
    if (!result.objectId) throw new ProtocolError("E9-003", `extract: node ${id} has no ancestor ${up}`);
    const { nodeId } = await cdp.send("DOM.requestNode", { objectId: result.objectId });
    cdpIds.set(key, nodeId);
    return nodeId;
  };

  const props = req.props;
  const all = props.map((_, i) => i);
  const beaconPick = req.beacon_props;
  const known = props.map((p) => p.name).concat(req.extra);

  // Applies (on = true) or clears the conditions of `mask` for node `n`.
  const apply = async (n, mask, on) => {
    const forced = new Map(); // cdp node id -> pseudo list
    for (let b = 0; b < n.conds.length; b++) {
      if (!(mask & (1 << b))) continue;
      const c = n.conds[b];
      if (c.kind === "class") await page.evaluate(([id, up, cls, v]) => window.__ui2x.setClass(id, up, cls, v), [n.id, c.up, c.class, on]);
      else if (c.kind === "disabled") await page.evaluate(([id, up, v]) => window.__ui2x.setDisabled(id, up, v), [n.id, c.up, on]);
      else {
        const nid = await cdpNode(n.id, c.up);
        if (!forced.has(nid)) forced.set(nid, []);
        forced.get(nid).push(c.kind);
      }
    }
    for (const [nodeId, list] of forced) await cdp.send("CSS.forcePseudoState", { nodeId, forcedPseudoClasses: on ? list : [] });
  };

  const readNode = (id, pick) => page.evaluate(([id, props, pick]) => window.__ui2x.read(window.__ui2x.el(id), props, pick), [id, props, pick]);
  const diff = (base, vals, pick) => {
    const set = [];
    pick.forEach((i, k) => { if (vals[k] !== base[i]) set.push([i, vals[k]]); });
    return set;
  };

  // Pass A: values.
  const cells = [];
  for (const [w, h] of req.cells) {
    await page.setViewportSize({ width: w, height: h });
    await settle(page);
    const out = [];
    for (const n of req.nodes) {
      const base = await readNode(n.id, all);
      const extra = await page.evaluate(([id, names]) => { const cs = getComputedStyle(window.__ui2x.el(id)); return names.map((p) => cs.getPropertyValue(p)); }, [n.id, req.extra]);
      const off = await page.evaluate(([id, known]) => window.__ui2x.offProfile(window.__ui2x.el(id), new Set(known)), [n.id, known]);
      const vars = [];
      for (let mask = 1; mask < (1 << n.conds.length); mask++) {
        await apply(n, mask, true);
        const vals = await readNode(n.id, all);
        await apply(n, mask, false);
        vars.push({ mask, set: diff(base, vals, all) });
      }
      out.push({ id: n.id, base, extra, off, vars });
    }
    cells.push({ viewport: [w, h], nodes: out });
  }

  // The beacon prefix: the smallest R such that no base color reads rgb(R, ...).
  const used = new Set();
  for (const c of cells) for (const n of c.nodes) {
    const strs = [...beaconPick.map((i) => n.base[i]), ...n.vars.flatMap((v) => v.set.map((s) => s[1]))];
    for (const s of strs) for (const m of String(s).matchAll(/rgba?\((\d+), (\d+), (\d+)\)/g)) used.add(Number(m[1]));
  }
  let r = 1;
  while (used.has(r) && r < 255) r++;

  const tokens = await page.evaluate((names) => window.__ui2x.tokenValues(names), req.tokens);
  const colorTokens = tokens.filter((t) => t.color !== null).map((t) => t.name);
  if (colorTokens.length > 65536) throw new ProtocolError("E9-003", "extract: more than 65536 color tokens");
  const sheet = colorTokens.length
    ? `:root:root{${colorTokens.map((name, i) => `${name}:rgb(${r}, ${i >> 8}, ${i & 255})`).join(";")}}`
    : "";

  // Pass B: the same reads of the token-capable properties with the beacons on.
  if (sheet) {
    await page.evaluate((t) => window.__ui2x.setBeacon(t), sheet);
    for (let ci = 0; ci < req.cells.length; ci++) {
      const [w, h] = req.cells[ci];
      await page.setViewportSize({ width: w, height: h });
      await settle(page);
      for (let ni = 0; ni < req.nodes.length; ni++) {
        const n = req.nodes[ni];
        const a = cells[ci].nodes[ni];
        const vb = await readNode(n.id, beaconPick);
        const base = diff(a.base, vb, beaconPick);
        const vars = [];
        for (let mask = 1; mask < (1 << n.conds.length); mask++) {
          const va = a.base.slice();
          for (const [i, v] of a.vars[mask - 1].set) va[i] = v;
          await apply(n, mask, true);
          const vals = await readNode(n.id, beaconPick);
          await apply(n, mask, false);
          vars.push({ mask, set: diff(va, vals, beaconPick) });
        }
        a.beacon = { base, vars };
      }
    }
    await page.evaluate(() => window.__ui2x.setBeacon(""));
  } else {
    for (const c of cells) for (const n of c.nodes) n.beacon = { base: [], vars: n.vars.map((v) => ({ mask: v.mask, set: [] })) };
  }
  await cdp.detach().catch(() => {});
  return { beacon: r, tokens, cells };
}
