// Browser-free source scan for binding checks; does not claim computed-style coverage.
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
const require = createRequire(new URL("../node/package.json", import.meta.url));
const { parse } = require("parse5");
const css = require("css-tree");
const names = ["tokens", "base", "hud", "onmap", "unit_lists", "minimap", "responsive", "tips"];
const part = n => readFileSync(new URL(`../../../skins/engraved/parts/${n}.html`, import.meta.url), "utf8");
const source = `<html><head><title>Engraved</title></head><body><div data-layer="map">${part("onmap")}</div><div data-layer="hud">${part("top")}${part("helm").replace('<div data-include="parts/minimap.html"></div>', part("minimap"))}${part("minimap_options")}${part("unit")}${part("unit_lists")}<div data-slot="mods" data-part="mods"></div></div><div data-layer="window">${part("city_list")}${part("unit_rename")}</div><div data-layer="tooltip">${part("top_panel_tips")}${part("on_map_tips")}${part("tips")}</div></body></html>`;
let id = 0;
function element(n) {
  if (n.nodeName === "#text") return {type: "text", id: id++, text: n.value, ws: !n.value.trim()};
  if (!n.tagName) return null;
  return { type: "element", id: id++, tag: n.tagName, ns: n.namespaceURI?.includes("svg") ? "svg" : undefined, src: "hud-group.html:1:1", attrs: (n.attrs || []).map(a => [a.name, a.value]), children: (n.content?.childNodes || n.childNodes || []).map(element).filter(Boolean) };
}
const dom = element(parse(source).childNodes.find(n => n.tagName === "html"));
function simplify(n) {
  const pos = {line: n.loc?.start.line || 1, col: n.loc?.start.column || 1};
  if (n.type === "Rule") return {type: "rule", ...pos, selector: css.generate(n.prelude), selectors: n.prelude.children.toArray().map(s => css.generate(s)), items: n.block.children.toArray().map(simplify)};
  if (n.type === "Atrule") return {type: "atrule", ...pos, name: n.name, prelude: n.prelude ? css.generate(n.prelude) : "", items: n.block?.children.toArray().map(simplify)};
  if (n.type === "Declaration") return {type: "decl", ...pos, prop: n.property, value: css.generate(n.value), important: !!n.important};
  return {type: n.type.toLowerCase(), ...pos, text: css.generate(n)};
}
const sheets = names.map(name => ({kind: "link", node: 0, file: `css/${name}.css`, rules: css.parse(readFileSync(new URL(`../../../skins/engraved/css/${name}.css`, import.meta.url), "utf8"), {positions:true}).children.toArray().map(simplify)}));
process.stdout.write(JSON.stringify({version: 1, state: "hud", entry: "index.html", viewport: [1366,768], compat: "CSS1Compat", dom, sheets}));
