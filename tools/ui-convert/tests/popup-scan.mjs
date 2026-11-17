// Source-only IR for popup profile tests. Computed layout remains the Chromium check's responsibility.
import fs from "node:fs";
import path from "node:path";
import { parse, parseFragment } from "../node/node_modules/parse5/dist/index.js";
import * as css from "../node/node_modules/css-tree/lib/index.js";
const root = process.argv[2];
let next = 0;
const sheets = [];
function reduce(n) {
  const pos = n.loc ? {line:n.loc.start.line,col:n.loc.start.column} : {};
  if(n.type === "Rule") return {type:"rule",...pos,selector:css.generate(n.prelude),selectors:n.prelude.children.toArray().map(css.generate),items:n.block.children.toArray().map(reduce)};
  if(n.type === "Atrule") return {type:"atrule",...pos,name:n.name,prelude:n.prelude?css.generate(n.prelude):"",items:n.block?.children.toArray().map(reduce)};
  if(n.type === "Declaration") return {type:"decl",...pos,prop:n.property,value:css.generate(n.value),important:!!n.important};
  return {type:"other"};
}
function convert(n,file) {
  if(n.nodeName === "#comment" || n.nodeName === "#documentType") return null;
  const id=next++;
  if(n.nodeName === "#text") return {type:"text",id,text:n.value,ws:! /[^ \t\n\r\f]/.test(n.value)};
  const attrs=(n.attrs??[]).map(a=>[a.name,a.value]);
  if(attrs.some(a=>a[0]==="data-ui2-dev")) return null;
  const loc=n.sourceCodeLocation;
  const e={type:"element",id,tag:n.tagName,attrs,src:file+":"+(loc?.startLine??1)+":"+(loc?.startCol??1),children:[]};
  if(n.namespaceURI?.includes("svg")) e.ns="svg";
  const include=attrs.find(a=>a[0]==="data-include")?.[1];
  const kids=include?parseFragment(fs.readFileSync(path.join(root,include),"utf8"),{sourceCodeLocationInfo:true}).childNodes:(n.content?.childNodes??n.childNodes??[]);
  e.children=kids.map(k=>convert(k,include??file)).filter(Boolean);
  if(n.tagName==="link" && attrs.some(a=>a[0]==="rel"&&a[1]==="stylesheet")) {
    const href=attrs.find(a=>a[0]==="href")[1];
    const ast=css.parse(fs.readFileSync(path.join(root,href),"utf8"),{positions:true});
    sheets.push({kind:"link",node:id,src:e.src,href,file:href,rules:ast.children.toArray().map(reduce)});
  }
  return e;
}
const doc=parse(fs.readFileSync(path.join(root,"index.html"),"utf8").replace("parts/notes.html","parts/notification_log_panel.html"),{sourceCodeLocationInfo:true});
const dom=convert(doc.childNodes.find(n=>n.tagName==="html"),"index.html");
process.stdout.write(JSON.stringify({version:1,entry:"index.html",viewport:[1920,1080],compat:"CSS1Compat",dom,sheets}));
