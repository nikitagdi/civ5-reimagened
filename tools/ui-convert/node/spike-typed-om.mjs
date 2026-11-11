// Spike: what does Chrome's Typed OM (element.computedStyleMap()) return for every property of the
// UI profile, compared with getComputedStyle()? Decides which API the extractor reads per property.
//
//   node spike-typed-om.mjs [--json <out.json>] [--verbose]
//
// Every case sets one value on a probe element inside a laid-out stage, then records:
//   typed   class + serialization of computedStyleMap().get(prop) (and getAll() for list values)
//   cs      getComputedStyle(prop) while the stage is displayed (resolved/used values)
//   hidden  getComputedStyle(prop) while the stage sits under display:none (computed values)
// Without --verbose only the interesting rows (typed differs from cs, or the property is special) are
// printed; the summary at the end lists, per property, whether typed and cs agree.
import fs from "node:fs";
import { launchChromium } from "./chromium.mjs";

const args = process.argv.slice(2);
const jsonOut = args.includes("--json") ? args[args.indexOf("--json") + 1] : null;
const verbose = args.includes("--verbose");

// Probe groups: how the probe element and its parent are set up so the property has a real effect.
const G = {
  plain: {},
  abs: { extra: "position:absolute" },
  flexItem: { parent: "display:flex" },
  flexBox: { extra: "display:flex", kids: 2 },
  gridBox: { extra: "display:grid", kids: 3 },
  gridItem: { parent: "display:grid;grid-template-columns:repeat(4,50px);grid-template-rows:repeat(4,30px)" },
  text: { text: "Gold 1 234 +12" },
  svg: { svg: true },
};

// property -> [group, [values...]]. Values are representative for the profile (SPEC-ui-profile section 4).
const LEN = ["12px", "50%", "auto", "calc(50% - 8px)", "2em", "1.5rem", "0", "calc(2em + 4px)"];
const LEN0 = ["12px", "50%", "0", "calc(50% - 8px)", "2em"];
const PROPS = {
  // --- layout
  display: [G.flexItem, ["none", "block", "flex", "grid", "inline", "inline-flex", "inline-block", "contents"]],
  position: [G.plain, ["static", "relative", "absolute", "fixed", "sticky"]],
  "z-index": [G.abs, ["auto", "5", "-3", "9999"]],
  "box-sizing": [G.plain, ["border-box", "content-box"]],
  top: [G.abs, [...LEN, "-10px", "calc(100% - 20px)"]],
  right: [G.abs, LEN],
  bottom: [G.abs, LEN],
  left: [G.abs, LEN],
  width: [G.plain, [...LEN, "calc(100% - 20px)", "min-content", "max-content", "fit-content", "10vw"]],
  height: [G.plain, [...LEN, "calc(100% - 20px)"]],
  "min-width": [G.plain, ["auto", "0", "30px", "20%", "min-content"]],
  "min-height": [G.plain, ["auto", "0", "30px", "20%"]],
  "max-width": [G.plain, ["none", "300px", "80%", "calc(100% - 4px)"]],
  "max-height": [G.plain, ["none", "300px", "80%"]],
  "aspect-ratio": [G.plain, ["auto", "16 / 9", "1", "auto 4 / 3"]],
  "margin-top": [G.plain, [...LEN, "-8px"]],
  "margin-right": [G.plain, [...LEN, "-8px"]],
  "margin-bottom": [G.plain, [...LEN, "-8px"]],
  "margin-left": [G.plain, [...LEN, "-8px"]],
  "padding-top": [G.plain, LEN0],
  "padding-right": [G.plain, LEN0],
  "padding-bottom": [G.plain, LEN0],
  "padding-left": [G.plain, LEN0],
  "overflow-x": [G.plain, ["visible", "hidden", "clip", "auto", "scroll"]],
  "overflow-y": [G.plain, ["visible", "hidden", "clip", "auto", "scroll"]],
  "scrollbar-width": [G.plain, ["auto", "none", "thin"]],
  "flex-direction": [G.flexBox, ["row", "row-reverse", "column", "column-reverse"]],
  "flex-wrap": [G.flexBox, ["nowrap", "wrap"]],
  "flex-grow": [G.flexItem, ["0", "1", "2.5"]],
  "flex-shrink": [G.flexItem, ["0", "1", "0.5"]],
  "flex-basis": [G.flexItem, ["auto", "0", "0%", "40px", "25%", "calc(25% + 4px)", "content", "2em"]],
  order: [G.flexItem, ["0", "-1", "3"]],
  "justify-content": [G.flexBox, ["normal", "flex-start", "flex-end", "start", "end", "center", "space-between", "space-around", "space-evenly"]],
  "align-items": [G.flexBox, ["normal", "stretch", "flex-start", "flex-end", "start", "end", "center", "baseline"]],
  "align-self": [G.flexItem, ["auto", "stretch", "center", "baseline", "flex-end"]],
  "align-content": [G.flexBox, ["normal", "flex-start", "center", "space-between"]],
  "justify-items": [G.gridBox, ["normal", "stretch", "center", "start"]],
  "justify-self": [G.gridItem, ["auto", "stretch", "center", "end"]],
  "row-gap": [G.flexBox, ["normal", "8px", "2em", "5%", "calc(4px + 2px)"]],
  "column-gap": [G.flexBox, ["normal", "8px", "2em", "5%", "calc(4px + 2px)"]],
  "grid-template-columns": [G.gridBox, ["none", "100px 1fr", "repeat(3, 1fr)", "minmax(40px, 1fr) auto 20%", "repeat(2, minmax(0, 1fr)) 60px", "min-content max-content", "repeat(auto-fill, 50px)", "[a] 50px [b]"]],
  "grid-template-rows": [G.gridBox, ["none", "40px 1fr", "repeat(2, 1fr)", "minmax(20px, auto)", "auto"]],
  "grid-auto-columns": [G.gridBox, ["auto", "50px", "1fr", "minmax(10px, 2fr)", "10%"]],
  "grid-auto-rows": [G.gridBox, ["auto", "30px", "1fr", "minmax(10px, auto)"]],
  "grid-auto-flow": [G.gridBox, ["row", "column", "row dense", "column dense"]],
  "grid-row-start": [G.gridItem, ["auto", "2", "span 2", "-1"]],
  "grid-row-end": [G.gridItem, ["auto", "3", "span 2"]],
  "grid-column-start": [G.gridItem, ["auto", "2", "span 3"]],
  "grid-column-end": [G.gridItem, ["auto", "4", "span 2"]],
  // --- appearance
  color: [G.text, ["#ebe2d2", "#abc", "rgb(10, 20, 30)", "rgba(10, 20, 30, 0.5)", "red", "transparent", "currentcolor", "color-mix(in srgb, red 50%, blue)", "hsl(120 50% 40%)", "var(--tok-bone)", "var(--tok-missing, #123456)"]],
  "background-color": [G.plain, ["#101820", "rgba(0,0,0,0.4)", "transparent", "currentcolor", "var(--tok-bone)", "color-mix(in srgb, var(--tok-bone) 40%, transparent)"]],
  opacity: [G.plain, ["1", "0.5", "0", "50%"]],
  visibility: [G.plain, ["visible", "hidden"]],
  "background-image": [G.plain, [
    "none",
    "linear-gradient(red, blue)",
    "linear-gradient(to top left, #123 0, #123 calc(100% - 10px), #456 calc(100% - 10px))",
    "linear-gradient(45deg, var(--tok-bone) 10%, transparent 90%)",
    "repeating-linear-gradient(90deg, #000 0 2px, #fff 2px 6px)",
    "conic-gradient(from 90deg at center, red, blue)",
    "repeating-conic-gradient(from 0deg at 50% 50%, #000 0 5deg, #fff 5deg 10deg)",
    "url(images/x.png)",
    "linear-gradient(red, blue), linear-gradient(#111, #222)",
    "radial-gradient(circle, red, blue)",
  ]],
  "background-size": [G.plain, ["auto", "cover", "contain", "12px 8px", "50% auto", "calc(100% - 4px) 10px", "auto, 10px", "var(--c, 14px)"]],
  "background-position-x": [G.plain, ["0%", "left", "center", "right", "10px", "calc(100% - 8px)", "right 10px", "50%"]],
  "background-position-y": [G.plain, ["0%", "top", "center", "bottom", "10px", "calc(100% - 8px)", "bottom 10px"]],
  "background-repeat": [G.plain, ["repeat", "no-repeat", "repeat-x", "repeat-y", "space", "no-repeat, repeat"]],
  "background-origin": [G.plain, ["padding-box", "border-box", "content-box"]],
  "background-clip": [G.plain, ["border-box", "padding-box", "content-box", "text"]],
  ...Object.fromEntries(["top", "right", "bottom", "left"].flatMap((s) => [
    [`border-${s}-width`, [G.plain, ["0", "1px", "2.5px", "thin", "medium", "thick", "0.1em"]]],
    [`border-${s}-style`, [G.plain, ["none", "solid", "dashed", "double"]]],
    [`border-${s}-color`, [G.plain, ["#456", "currentcolor", "transparent", "var(--tok-bone)", "rgba(0,0,0,.3)"]]],
  ])),
  ...Object.fromEntries(["top-left", "top-right", "bottom-right", "bottom-left"].map((c) => [
    `border-${c}-radius`, [G.plain, ["0", "6px", "50%", "10px 20px", "50% 25%", "calc(50% - 4px)", "1em", "999px"]],
  ])),
  "border-image-source": [G.plain, ["none", "url(images/frame.png)", "linear-gradient(red, blue)"]],
  "border-image-slice": [G.plain, ["100%", "12", "12 fill", "10 20 30 40", "25% 10%"]],
  "border-image-width": [G.plain, ["1", "12px", "2 4px", "auto", "10%"]],
  "border-image-repeat": [G.plain, ["stretch", "repeat", "round", "stretch round"]],
  "border-image-outset": [G.plain, ["0", "2px", "1 2px", "1"]],
  "outline-style": [G.plain, ["none", "solid", "auto"]],
  "outline-width": [G.plain, ["0", "2px", "medium", "0.1em"]],
  "outline-color": [G.plain, ["#456", "currentcolor", "var(--tok-bone)", "invert"]],
  "outline-offset": [G.plain, ["0", "2px", "-2px", "0.5em"]],
  "box-shadow": [G.plain, [
    "none",
    "0 1px 2px #000",
    "inset 0 0 0 1px rgba(255,255,255,.1)",
    "0 0 4px 1px var(--tok-bone)",
    "inset 0 1px 0 #fff, 0 2px 6px rgba(0,0,0,.5)",
    "0 0 0 calc(2px + 1px) red",
    "0 0 1em red",
  ]],
  "text-shadow": [G.text, ["none", "0 1px 1px #000", "1px 1px 2px var(--tok-bone)", "0 0 0.5em red"]],
  "clip-path": [G.plain, [
    "none",
    "polygon(0 0, 100% 0, 100% calc(100% - 10px), calc(100% - 10px) 100%, 0 100%)",
    "polygon(10px 0, 100% 0, 100% 100%, 0 100%)",
    "polygon(50% 0, 100% 50%, 50% 100%, 0 50%)",
    "circle(50%)",
    "inset(0 round 4px)",
  ]],
  transform: [G.plain, [
    "none",
    "translate(50%, 10px)",
    "translateX(-50%)",
    "translateY(calc(100% - 4px))",
    "rotate(45deg)",
    "rotate(0.25turn)",
    "scale(1.5)",
    "scale(0.5, 2)",
    "scaleX(2)",
    "translate(10px, 20px) rotate(10deg) scale(1.1)",
    "translate(50%, 50%) rotate(-90deg)",
    "matrix(1, 0, 0, 1, 5, 6)",
    "skewX(10deg)",
    "translate(1em, 2em)",
  ]],
  "transform-origin": [G.plain, ["50% 50%", "0 0", "left top", "center", "10px 20px", "calc(100% - 4px) 50%", "100% 100%", "right bottom"]],
  cursor: [G.plain, ["auto", "default", "pointer", "text", "move", "grab", "grabbing", "not-allowed", "url(c.png), pointer"]],
  "pointer-events": [G.plain, ["auto", "none"]],
  "transition-property": [G.plain, ["none", "all", "color", "color, background-color", "width", "opacity, transform"]],
  "transition-duration": [G.plain, ["0s", "200ms", "0.2s", "1s", "100ms, 2s"]],
  "transition-delay": [G.plain, ["0s", "100ms", "0.5s", "-50ms"]],
  "transition-timing-function": [G.plain, ["ease", "linear", "ease-in", "ease-out", "ease-in-out", "cubic-bezier(.2, .8, .2, 1)", "steps(4, end)", "ease, linear"]],
  // --- text
  "font-family": [G.text, ["Fira Sans", "'Fira Sans', sans-serif", "Alegreya, 'Times New Roman', serif", "var(--sans)", "system-ui"]],
  "font-size": [G.text, ["12px", "1.5em", "120%", "1rem", "larger", "small", "calc(10px + 2px)"]],
  "font-weight": [G.text, ["normal", "bold", "400", "600", "700", "lighter", "bolder", "calc(300 + 100)"]],
  "font-style": [G.text, ["normal", "italic", "oblique"]],
  "font-variant-numeric": [G.text, ["normal", "tabular-nums", "tabular-nums lining-nums", "proportional-nums oldstyle-nums", "slashed-zero"]],
  "line-height": [G.text, ["normal", "1.5", "20px", "150%", "1.2em", "calc(10px + 4px)"]],
  "letter-spacing": [G.text, ["normal", "1px", "0.05em", "-0.5px", "2%"]],
  "word-spacing": [G.text, ["normal", "2px", "0.2em"]],
  "text-align": [G.text, ["start", "end", "left", "right", "center", "justify"]],
  "text-indent": [G.text, ["0", "10px", "2em", "5%", "10px each-line"]],
  "white-space-collapse": [G.text, ["collapse", "preserve", "preserve-breaks", "break-spaces"]],
  "text-wrap-mode": [G.text, ["wrap", "nowrap"]],
  "text-wrap-style": [G.text, ["auto", "balance", "pretty"]],
  "text-overflow": [G.text, ["clip", "ellipsis"]],
  "overflow-wrap": [G.text, ["normal", "break-word", "anywhere"]],
  "word-break": [G.text, ["normal", "break-word", "break-all", "keep-all"]],
  "text-transform": [G.text, ["none", "uppercase", "capitalize"]],
  "text-decoration-line": [G.text, ["none", "underline", "underline line-through"]],
  "vertical-align": [G.text, ["baseline", "middle", "top", "10px", "50%", "super"]],
  direction: [G.text, ["ltr", "rtl"]],
  "writing-mode": [G.text, ["horizontal-tb", "vertical-rl"]],
  // --- svg icons
  fill: [G.svg, ["none", "currentcolor", "#f00", "var(--tok-bone)", "url(#g)"]],
  stroke: [G.svg, ["none", "currentcolor", "#00f", "var(--tok-bone)"]],
  "stroke-width": [G.svg, ["1", "2px", "0.5", "10%", "1.5em"]],
  "stroke-linecap": [G.svg, ["butt", "round", "square"]],
  "stroke-linejoin": [G.svg, ["miter", "round", "bevel"]],
  // --- off-profile properties the extractor must still notice (initial-value comparison, E2)
  float: [G.plain, ["none", "left", "right"]],
  "font-feature-settings": [G.text, ["normal", "'tnum' 1", "'liga' 0"]],
  "animation-name": [G.plain, ["none", "spin"]],
  filter: [G.plain, ["none", "blur(2px)", "drop-shadow(0 0 2px red)"]],
  translate: [G.plain, ["none", "10px 20px", "50% 0"]],
  rotate: [G.plain, ["none", "45deg"]],
  scale: [G.plain, ["none", "1.5"]],
  appearance: [G.plain, ["auto", "none"]],
  "user-select": [G.text, ["auto", "none"]],
  "-webkit-font-smoothing": [G.text, ["auto", "antialiased"]],
};

// Shorthands: does computedStyleMap().get(shorthand) work, or must longhands be read?
const SHORTHANDS = {
  margin: "4px 8px 12px 16px", padding: "1px 2px", border: "1px solid red", "border-radius": "4px 8px / 2px 6px",
  background: "linear-gradient(red, blue) no-repeat 0 0 / 10px 10px, #123", flex: "1 1 0%", gap: "4px 8px", inset: "0 10px",
  overflow: "hidden auto", transition: "color .2s ease, background-color 100ms linear", font: "italic 700 14px/1.2 Fira Sans",
  "grid-template": "auto 1fr / 40px 1fr", "place-items": "center start", outline: "1px solid red", "text-decoration": "underline",
};

const results = await run();
report(results);
if (jsonOut) fs.writeFileSync(jsonOut, JSON.stringify(results, null, 2) + "\n");

async function run() {
  const browser = await launchChromium();
  const page = await browser.newPage({ viewport: { width: 800, height: 600 }, deviceScaleFactor: 1 });
  await page.setContent(`<!doctype html><html><head><style>
    :root{--tok-bone:#ebe2d2;--c:14px;--sans:'Fira Sans',sans-serif}
    body{margin:0;font:16px/1.2 sans-serif}
    #wrap{display:block}
    #stage{position:relative;width:400px;height:300px;font-size:20px}
  </style></head><body><div id="wrap"><div id="stage"><div id="parent"></div></div></div></body></html>`);
  const version = browser.version();
  const out = await page.evaluate(async ({ PROPS, SHORTHANDS }) => {
    const wrap = document.getElementById("wrap");
    const parent = document.getElementById("parent");
    const SVGNS = "http://www.w3.org/2000/svg";

    function typedInfo(map, prop) {
      let v;
      try { v = map.get(prop); } catch (e) { return { err: String(e) }; }
      if (v === null || v === undefined) return { cls: null };
      const info = { cls: v.constructor.name, str: v.toString() };
      if (v instanceof CSSUnitValue) { info.unit = v.unit; info.num = v.value; }
      if (v instanceof CSSMathValue) info.op = v.operator;
      if (v instanceof CSSTransformValue) info.items = [...v].map((t) => `${t.constructor.name}:${t}`);
      let all = [];
      try { all = map.getAll(prop); } catch (e) { /* ignore */ }
      info.n = all.length;
      if (all.length > 1) info.all = all.map((x) => `${x.constructor.name}:${x}`);
      return info;
    }

    function setup(group) {
      parent.removeAttribute("style");
      parent.textContent = "";
      if (group.parent) parent.style.cssText = group.parent;
      let el;
      if (group.svg) {
        const svg = document.createElementNS(SVGNS, "svg");
        svg.setAttribute("width", "20"); svg.setAttribute("height", "20");
        el = document.createElementNS(SVGNS, "path");
        el.setAttribute("d", "M0 0h10v10z");
        svg.appendChild(el);
        parent.appendChild(svg);
      } else {
        el = document.createElement("div");
        if (group.text) el.textContent = group.text;
        parent.appendChild(el);
        for (let i = 0; i < (group.kids || 0); i++) { const k = document.createElement("i"); k.style.cssText = "display:block;width:20px;height:10px"; el.appendChild(k); }
      }
      return el;
    }

    const rows = [];
    for (const [prop, [group, values]] of Object.entries(PROPS)) {
      for (const value of values) {
        const el = setup(group);
        wrap.style.display = "block";
        // property under test is set last so it wins over the group's helper declarations
        const base = (group.extra || "");
        el.style.cssText = base;
        el.style.setProperty(prop, value);
        const accepted = el.style.getPropertyValue(prop) !== "";
        const row = { prop, value, accepted };
        if (!accepted) { rows.push(row); continue; }
        row.typed = typedInfo(el.computedStyleMap(), prop);
        row.cs = getComputedStyle(el).getPropertyValue(prop);
        wrap.style.display = "none";
        row.hidden = getComputedStyle(el).getPropertyValue(prop);
        row.typedHidden = typedInfo(el.computedStyleMap(), prop).str ?? null;
        wrap.style.display = "block";
        rows.push(row);
      }
    }

    // shorthands
    const shorts = [];
    for (const [prop, value] of Object.entries(SHORTHANDS)) {
      const el = setup({}); el.style.setProperty(prop, value);
      const t = typedInfo(el.computedStyleMap(), prop);
      shorts.push({ prop, value, accepted: el.style.getPropertyValue(prop) !== "", typed: t, cs: getComputedStyle(el).getPropertyValue(prop) });
    }

    // custom properties and tokens
    const custom = {};
    {
      const el = setup({});
      el.style.cssText = "--n:12px;--col:#ebe2d2;--ref:var(--tok-bone);color:var(--ref);width:var(--n);background-image:linear-gradient(var(--ref),red)";
      const m = el.computedStyleMap();
      for (const p of ["--n", "--col", "--ref", "--tok-bone", "--missing"]) custom[p] = typedInfo(m, p);
      custom.cs = Object.fromEntries(["--n", "--col", "--ref", "color", "width", "background-image"].map((p) => [p, getComputedStyle(el).getPropertyValue(p)]));
      custom.typedConsumers = Object.fromEntries(["color", "width", "background-image"].map((p) => [p, typedInfo(m, p)]));
    }

    // enumeration support
    const enumeration = { cs: getComputedStyle(parent).length, typedSize: parent.computedStyleMap().size };

    // elements without a box
    const hiddenRoot = {};
    {
      const el = setup({});
      el.style.cssText = "width:50%;height:calc(100% - 10px);margin-left:auto;top:5%;position:absolute;display:none";
      const m = el.computedStyleMap();
      for (const p of ["width", "height", "margin-left", "top", "display"]) hiddenRoot[p] = { typed: typedInfo(m, p).str, cs: getComputedStyle(el).getPropertyValue(p) };
      const det = document.createElement("div");
      det.style.cssText = "width:50%";
      hiddenRoot.detached = { typed: (() => { try { return det.computedStyleMap().get("width")?.toString() ?? null; } catch (e) { return String(e); } })(), cs: getComputedStyle(det).getPropertyValue("width") };
    }

    // transitions: a value read mid-flight is neither the old nor the new value
    const transition = {};
    {
      const el = setup({});
      el.style.cssText = "color:rgb(0, 0, 0);transition:color 2s linear";
      el.computedStyleMap(); getComputedStyle(el).color;
      el.style.color = "rgb(255, 255, 255)";
      transition.justAfterChange = el.computedStyleMap().get("color").toString();
      await new Promise((r) => setTimeout(r, 600));
      transition.midFlight = el.computedStyleMap().get("color").toString();
      const st = document.createElement("style");
      st.textContent = "*{transition:none!important}";
      document.head.appendChild(st);
      transition.withKillSheet = el.computedStyleMap().get("color").toString();
      st.remove();
    }
    // multi-value (comma list) longhands: does get() return the whole list or only the first item?
    const lists = [];
    {
      const el = setup({});
      el.style.cssText = "background-image:linear-gradient(red,blue),linear-gradient(#111,#222);background-size:10px 20px,50% auto;" +
        "background-repeat:no-repeat,repeat-x;background-position-x:0%,10px;box-shadow:0 0 1px red,0 1px 2px blue;" +
        "transition:color .1s,width .2s ease-in;font-family:a,b";
      const m = el.computedStyleMap();
      for (const k of ["background-size", "background-repeat", "background-position-x", "box-shadow", "transition-property", "transition-duration", "transition-timing-function", "font-family"]) {
        lists.push({ prop: k, cls: m.get(k).constructor.name, get: String(m.get(k)), all: m.getAll(k).map(String), cs: getComputedStyle(el).getPropertyValue(k) });
      }
    }
    return { rows, shorts, custom, enumeration, hiddenRoot, transition, lists };
  }, { PROPS: Object.fromEntries(Object.entries(PROPS).map(([k, [g, v]]) => [k, [g, v]])), SHORTHANDS });

  // CDP: forced :hover must be visible to computedStyleMap (variants pass of the extractor)
  const hover = await hoverProbe(page);
  await browser.close();
  return { chromium: version, ...out, hover };
}

async function hoverProbe(page) {
  await page.evaluate(() => {
    const st = document.createElement("style");
    st.textContent = "#hv{color:rgb(1, 2, 3);background-color:rgb(9, 9, 9)} #hv:hover{color:rgb(10, 20, 30)} #hv:disabled{opacity:0.5}";
    document.head.appendChild(st);
    const b = document.createElement("button"); b.id = "hv"; b.textContent = "x"; document.body.appendChild(b);
  });
  const cdp = await page.context().newCDPSession(page);
  await cdp.send("DOM.enable"); await cdp.send("CSS.enable");
  const { root } = await cdp.send("DOM.getDocument");
  const { nodeId } = await cdp.send("DOM.querySelector", { nodeId: root.nodeId, selector: "#hv" });
  const read = () => page.evaluate(() => { const e = document.getElementById("hv"); return e.computedStyleMap().get("color").toString(); });
  const before = await read();
  await cdp.send("CSS.forcePseudoState", { nodeId, forcedPseudoClasses: ["hover"] });
  const forced = await read();
  await cdp.send("CSS.forcePseudoState", { nodeId, forcedPseudoClasses: [] });
  const after = await read();
  await cdp.detach();
  return { before, forced, after };
}

function short(t) {
  if (!t) return "-";
  if (t.err) return `ERR ${t.err}`;
  if (t.cls === null) return "null";
  return `${t.cls}(${t.str})`;
}

function report(r) {
  console.log(`chromium ${r.chromium}`);
  const byProp = new Map();
  for (const row of r.rows) {
    if (!byProp.has(row.prop)) byProp.set(row.prop, []);
    byProp.get(row.prop).push(row);
  }
  console.log("\n== summary per property: classes seen by Typed OM, and agreement with getComputedStyle ==");
  for (const [prop, rows] of byProp) {
    const ok = rows.filter((x) => x.accepted);
    const classes = [...new Set(ok.map((x) => x.typed.cls))].join("|");
    const same = ok.filter((x) => x.typed.str === x.cs).length;
    const hiddenSame = ok.filter((x) => x.typed.str === x.hidden).length;
    const rejected = rows.filter((x) => !x.accepted).map((x) => x.value);
    console.log(`${prop.padEnd(28)} cases ${String(ok.length).padStart(2)} | classes ${classes} | typed==cs ${same}/${ok.length} | typed==cs(hidden) ${hiddenSame}/${ok.length}${rejected.length ? ` | rejected: ${rejected.join(" ; ")}` : ""}`);
  }
  console.log("\n== rows where Typed OM and getComputedStyle (displayed) differ (typed | cs | cs under display:none) ==");
  for (const row of r.rows) {
    if (!row.accepted) continue;
    if (!verbose && row.typed.str === row.cs) continue;
    console.log(`${row.prop} : ${row.value}\n    typed  ${short(row.typed)}${row.typed.items ? ` items=${JSON.stringify(row.typed.items)}` : ""}${row.typed.all ? ` all=${JSON.stringify(row.typed.all)}` : ""}\n    cs     ${row.cs}\n    hidden ${row.hidden}`);
  }
  console.log("\n== shorthands ==");
  for (const s of r.shorts) console.log(`${s.prop} : ${s.value} -> accepted=${s.accepted} typed=${short(s.typed)} cs=${s.cs}`);
  console.log("\n== custom properties ==");
  console.log(JSON.stringify(r.custom, null, 1));
  console.log("\n== comma lists: get() vs getAll() vs getComputedStyle ==");
  for (const l of r.lists) console.log(`${l.prop.padEnd(28)} ${l.cls} get=${l.get} | getAll=${JSON.stringify(l.all)} | cs=${l.cs}`);
  console.log("\n== enumeration ==", JSON.stringify(r.enumeration));
  console.log("== element without a box (display:none) and detached ==", JSON.stringify(r.hiddenRoot));
  console.log("== transition mid-flight vs kill sheet ==", JSON.stringify(r.transition));
  console.log("== CDP forcePseudoState(hover) seen by computedStyleMap ==", JSON.stringify(r.hover));
}
