// Style extraction: Chromium as the style compiler.
//
// The request is the converter's plan (Rust, src/plan.rs); this file only reads what Chrome
// computed and never interprets a value (numbers stay Chrome's strings).
//
//   extract {cells:[[w,h]...], props:[{name, api:"T"|"C"}...], beacon_props:[i...], extra:[name...],
//            tokens:[name...], nodes:[{id, conds:[{up, kind, class?}...]}...]}
//   -> {beacon, tokens:[{name, value, color}], cells:[{viewport, nodes:[{id, base:[...], extra:[...],
//       off:[[name, value, initial]...], vars:[{mask, set:[[i, value]...]}...],
//       beacon:{base:[[i, value]...], vars:[{mask, set:[[i, value]...]}...]}, size?:[w, h]}]}]}
//
// `size` (CSS px of the border box, base state) is there only for a node with a `clip-path`.
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
import { entryUrl, loadPage } from "./ir.mjs";

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
  // The values of `pick`, the transition longhands left null: they are read with the kill-sheet off, once for
  // many nodes (`fillTrans`), because switching the sheet restyles the whole document.
  const read = (e, props, pick) => {
    const out = [];
    let map = null;
    let cs = null;
    for (const i of pick) {
      const p = props[i];
      if (p.name.startsWith("transition-")) { out.push(null); continue; }
      if (p.api === "T") {
        map = map || e.computedStyleMap();
        const v = map.get(p.name);
        out.push(v == null ? "" : v.toString());
      } else {
        cs = cs || getComputedStyle(e);
        out.push(cs.getPropertyValue(p.name));
      }
    }
    return out;
  };
  const isTrans = (i) => plan.props[i].name.startsWith("transition-");
  // The transition longhands of `pick` into `out` (a fresh list of nulls when there is none); the kill-sheet must
  // be off.
  const fillTrans = (e, pick, out) => {
    const res = out || pick.map(() => null);
    const cs = getComputedStyle(e);
    pick.forEach((i, k) => { if (isTrans(i)) res[k] = cs.getPropertyValue(plan.props[i].name); });
    return res;
  };
  const killOff = (off) => { kill.sheet.disabled = off; };
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
  // Turning a condition off restores the element as the page had it (a static class or `disabled` stays).
  const setClass = (id, up, cls, on) => {
    const a = ancestor(id, up);
    if (!a) return;
    if (a.__ui2cls === undefined) a.__ui2cls = new Set(a.classList);
    a.classList.toggle(cls, on || a.__ui2cls.has(cls));
  };
  const setDisabled = (id, up, on) => {
    const a = ancestor(id, up);
    if (!a) return;
    if (a.__ui2dis === undefined) a.__ui2dis = a.hasAttribute("disabled");
    if (on || a.__ui2dis) a.setAttribute("disabled", ""); else a.removeAttribute("disabled");
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
  // The plan's property list, sent once (every read below names properties by index).
  let plan = null;
  const setPlan = (props, extra) => {
    plan = { props, extra, all: props.map((_, i) => i), known: new Set(props.map((p) => p.name).concat(extra)) };
  };
  // The class and `disabled` conditions of `mask` on (or off); pseudo-classes are forced over CDP.
  const applyIn = (id, conds, mask, on) => {
    conds.forEach((c, b) => {
      if (!(mask & (1 << b))) return;
      if (c.kind === "class") setClass(id, c.up, c.class, on);
      else if (c.kind === "disabled") setDisabled(id, c.up, on);
    });
  };
  // Base reads of many nodes at once: values, extra values, off-profile differences.
  const readBase = (ids) => {
    const rows = ids.map((id) => {
      const e = nodes[id];
      const cs = getComputedStyle(e);
      return { base: read(e, plan.props, plan.all), extra: plan.extra.map((p) => cs.getPropertyValue(p)), off: offProfile(e, plan.known) };
    });
    if (plan.all.some(isTrans)) {
      killOff(true);
      ids.forEach((id, k) => fillTrans(nodes[id], plan.all, rows[k].base));
      killOff(false);
    }
    return rows;
  };
  // One variant: its in-page conditions on, the values read, the conditions off. `pick` null = every property.
  // `trans`: only the transition longhands (the caller switched the kill-sheet off).
  const readMasked = (id, conds, mask, pick, trans = false) => {
    const p = pick || plan.all;
    applyIn(id, conds, mask, true);
    const v = trans ? fillTrans(nodes[id], p, null) : read(nodes[id], plan.props, p);
    applyIn(id, conds, mask, false);
    return v;
  };
  const hasTrans = (pick) => (pick || plan.all).some(isTrans);
  const readJobs = (jobs, pick) => {
    const out = jobs.map((j) => readMasked(j.id, j.conds, j.mask, pick));
    if (hasTrans(pick)) {
      killOff(true);
      jobs.forEach((j, k) => {
        const t = readMasked(j.id, j.conds, j.mask, pick, true);
        t.forEach((v, i) => { if (v !== null) out[k][i] = v; });
      });
      killOff(false);
    }
    return out;
  };
  window.__ui2x = { el, ancestor, read, offProfile, setClass, setDisabled, tokenValues, setBeacon, ids, setPlan, readBase, readMasked, readJobs, killOff, hasTrans };
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
export async function extractPage({ page, server, entry, req, state = "" }) {
  checkRequest(req);
  server.reset();
  await page.setViewportSize({ width: req.cells[0][0], height: req.cells[0][1] });
  // The same page as the scan: extract mode of the preview, the scan's sample state.
  await loadPage(page, entryUrl(server, entry, "extract", state));
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
  const clipAt = props.findIndex((p) => p.name === "clip-path");
  await page.evaluate(([props, extra]) => window.__ui2x.setPlan(props, extra), [props, req.extra]);

  // The variants to read: one job per (node, non-empty mask). A job whose mask holds only classes and
  // `disabled` runs inside the page; one with a pseudo-class needs CDP to force it.
  const jobs = [];
  for (const n of req.nodes) {
    for (let mask = 1; mask < (1 << n.conds.length); mask++) {
      const pseudo = n.conds.some((c, b) => (mask & (1 << b)) && PSEUDO.has(c.kind));
      jobs.push({ n, mask, pseudo });
    }
  }
  // The CDP node ids of the elements that get a forced state, resolved once (requests pipelined).
  const keys = new Set();
  for (const j of jobs) if (j.pseudo) j.n.conds.forEach((c, b) => { if ((j.mask & (1 << b)) && PSEUDO.has(c.kind)) keys.add(`${j.n.id}/${c.up}`); });
  await Promise.all([...keys].map((k) => cdpNode(...k.split("/").map(Number))));

  // Reads every job with `pick` (null = all properties); returns the value lists in job order. The in-page jobs
  // go in one call; the CDP ones are pipelined on the CDP session, whose commands run in order: force the
  // pseudo-classes, read (classes toggled in the same call), clear.
  const readVariants = async (pick) => {
    const out = new Array(jobs.length);
    const local = jobs.map((j, k) => [j, k]).filter(([j]) => !j.pseudo);
    const vals = await page.evaluate(([list, pick]) => window.__ui2x.readJobs(list, pick),
      [local.map(([j]) => ({ id: j.n.id, conds: j.n.conds, mask: j.mask })), pick]);
    local.forEach(([, k], i) => { out[k] = vals[i]; });
    const evalCdp = (expression) => cdp.send("Runtime.evaluate", { expression, returnByValue: true }).then((r) => {
      if (r.exceptionDetails) throw new ProtocolError("E9-003", `extract: ${r.exceptionDetails.text}`);
      return r.result.value;
    });
    // Pass 1: everything but the transition longhands; pass 2 (kill-sheet off): those.
    const trans = pick === null ? props.some((p) => p.name.startsWith("transition-")) : pick.some((i) => props[i].name.startsWith("transition-"));
    for (const phase of trans ? [false, true] : [false]) {
    const sends = [];
    if (phase) sends.push(evalCdp("window.__ui2x.killOff(true)"));
    jobs.forEach((j, k) => {
      if (!j.pseudo) return;
      const forced = new Map();
      j.n.conds.forEach((c, b) => {
        if (!(j.mask & (1 << b)) || !PSEUDO.has(c.kind)) return;
        const nid = cdpIds.get(`${j.n.id}/${c.up}`);
        if (!forced.has(nid)) forced.set(nid, []);
        forced.get(nid).push(c.kind);
      });
      for (const [nodeId, list] of forced) sends.push(cdp.send("CSS.forcePseudoState", { nodeId, forcedPseudoClasses: list }));
      const expr = `window.__ui2x.readMasked(${j.n.id}, ${JSON.stringify(j.n.conds)}, ${j.mask}, ${JSON.stringify(pick)}, ${phase})`;
      sends.push(evalCdp(expr).then((v) => {
        if (!phase) out[k] = v;
        else v.forEach((x, i) => { if (x !== null) out[k][i] = x; });
      }));
      for (const nodeId of forced.keys()) sends.push(cdp.send("CSS.forcePseudoState", { nodeId, forcedPseudoClasses: [] }));
    });
    if (phase) sends.push(evalCdp("window.__ui2x.killOff(false)"));
    await Promise.all(sends);
    }
    return out;
  };
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
    const bases = await page.evaluate((ids) => window.__ui2x.readBase(ids), req.nodes.map((n) => n.id));
    const variants = await readVariants(null);
    const out = req.nodes.map((n, i) => ({ id: n.id, base: bases[i].base, extra: bases[i].extra, off: bases[i].off, vars: [] }));
    const at = new Map(req.nodes.map((n, i) => [n, i]));
    jobs.forEach((j, k) => {
      const node = out[at.get(j.n)];
      node.vars.push({ mask: j.mask, set: diff(node.base, variants[k], all) });
    });
    for (const node of out) {
      // A clipped node: the size of its border box (the clip polygon's reference box), so the converter can
      // check the polygon at the size it has.
      if (clipAt >= 0 && (node.base[clipAt] !== "none" || node.vars.some((v) => v.set.some(([i]) => i === clipAt)))) {
        node.size = await page.evaluate((id) => {
          const e = window.__ui2x.el(id);
          if (e instanceof HTMLElement) return [e.offsetWidth, e.offsetHeight];
          const r = e.getBoundingClientRect();
          return [r.width, r.height];
        }, node.id);
      }
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
      const bases = await page.evaluate(([ids, pick]) => ids.map((id) => window.__ui2x.readMasked(id, [], 0, pick)), [req.nodes.map((n) => n.id), beaconPick]);
      const variants = await readVariants(beaconPick);
      const at = new Map(req.nodes.map((n, i) => [n, i]));
      cells[ci].nodes.forEach((a, ni) => { a.beacon = { base: diff(a.base, bases[ni], beaconPick), vars: [] }; });
      jobs.forEach((j, k) => {
        const a = cells[ci].nodes[at.get(j.n)];
        const va = a.base.slice();
        for (const [i, v] of a.vars[j.mask - 1].set) va[i] = v;
        a.beacon.vars.push({ mask: j.mask, set: diff(va, variants[k], beaconPick) });
      });
    }
    await page.evaluate(() => window.__ui2x.setBeacon(""));
  } else {
    for (const c of cells) for (const n of c.nodes) n.beacon = { base: [], vars: n.vars.map((v) => ({ mask: v.mask, set: [] })) };
  }
  await cdp.detach().catch(() => {});
  return { beacon: r, tokens, cells };
}
