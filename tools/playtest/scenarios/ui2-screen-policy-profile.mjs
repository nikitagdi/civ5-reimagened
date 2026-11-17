// Source-only profile probe. No browser expansion, computed styles or layout assertions.
// Usage: node tools/playtest/scenarios/ui2-screen-policy-profile.mjs target/policy-source-ir.json
import fs from 'node:fs';
import path from 'node:path';
import {pathToFileURL} from 'node:url';
const root=process.cwd(), dir=path.join(root,'skins/engraved');
const parse5=await import(pathToFileURL(path.join(root,'tools/ui-convert/node/node_modules/parse5/dist/index.js')));
const css=await import(pathToFileURL(path.join(root,'tools/ui-convert/node/node_modules/css-tree/lib/index.js')));
let next=0; const sheets=[];
function simplify(n) {
 const pos={line:n.loc?.start.line??1,col:n.loc?.start.column??1};
 if(n.type==='Rule')return {type:'rule',...pos,selector:css.generate(n.prelude),selectors:n.prelude.children.toArray().map(s=>css.generate(s)),items:n.block.children.toArray().map(simplify)};
 if(n.type==='Atrule')return {type:'atrule',...pos,name:n.name,prelude:n.prelude?css.generate(n.prelude):'',items:n.block?n.block.children.toArray().map(simplify):undefined};
 if(n.type==='Declaration')return {type:'decl',...pos,prop:n.property,value:css.generate(n.value),important:!!n.important};
 return {type:'other'};
}
function children(n,file,svg=false){return (n.childNodes??[]).flatMap(c=>build(c,file,svg));}
function build(n,file,svg=false){
 if(n.nodeName==='#comment'||n.nodeName==='#documentType')return [];
 if(n.nodeName==='#text')return [{type:'text',id:next++,text:n.value,ws:!n.value.trim()}];
 const attrs=n.attrs??[];const get=k=>attrs.find(a=>a.name===k)?.value;
 if(get('data-ui2-dev')!==undefined)return [];
 const include=get('data-include');
 if(include){const frag=parse5.parseFragment(fs.readFileSync(path.join(dir,include),'utf8'),{sourceCodeLocationInfo:true});return children(frag,include);}
 const el={type:'element',id:next++,tag:n.tagName,attrs:attrs.map(a=>[a.name,a.value]),src:`${file}:${n.sourceCodeLocation?.startLine??1}:${n.sourceCodeLocation?.startCol??1}`,children:[]};
 svg=svg||n.tagName==='svg';if(svg)el.ns='svg';
 if(n.tagName==='link'&&get('rel')==='stylesheet'){
 const href=get('href'),ast=css.parse(fs.readFileSync(path.join(dir,href),'utf8'),{positions:true});sheets.push({kind:'link',node:el.id,file:href,href,rules:ast.children.toArray().map(simplify)});
 }
 el.children=n.tagName==='template'?children(n.content,file,svg):children(n,file,svg);
 return [el];
}
const doc=parse5.parse(fs.readFileSync(path.join(dir,'index.html'),'utf8'),{sourceCodeLocationInfo:true});
const dom=build(doc.childNodes.find(n=>n.tagName==='html'),'index.html')[0];
const result=JSON.stringify({v:1,preview:false,scan:{version:1,state:'hud',entry:'index.html',viewport:[1920,1080],compat:'CSS1Compat',dom,sheets}});
if(process.argv[2]==='-') process.stdout.write(result);
else { fs.mkdirSync(path.dirname(process.argv[2]),{recursive:true}); fs.writeFileSync(process.argv[2],result); }
