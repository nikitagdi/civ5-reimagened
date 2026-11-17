// Build a source-only IR for static profile checks when Chromium cannot start.
// This does not claim computed-style, font rasterization, or layout coverage.
import fs from 'node:fs';
import path from 'node:path';
import { createRequire } from 'node:module';
import { pathToFileURL } from 'node:url';
const require = createRequire(new URL('../node/package.json', import.meta.url));
const { parse, parseFragment } = await import(pathToFileURL(require.resolve('parse5')));
const csstree = await import(pathToFileURL(require.resolve('css-tree')));
const root = path.resolve(import.meta.dirname, '../../../skins/engraved');
const irSource = fs.readFileSync(path.resolve(import.meta.dirname, '../node/ir.mjs'), 'utf8');
const simplifySource = irSource.slice(irSource.indexOf('function simplify('), irSource.indexOf('function splitSrc('));
const parseCss = Function('csstree', `${simplifySource}; return parseCss;`)(csstree);
let next = 0;
const sheets = [];
function convert(n, file) {
  if (n.nodeName === '#text') return { type: 'text', id: next++, text: n.value, ws: /^\s*$/.test(n.value) };
  if (!n.tagName) return null;
  const attrs = (n.attrs ?? []).map(a => [a.name, a.value]);
  const get = k => attrs.find(a => a[0] === k)?.[1];
  if (get('data-ui2-dev') !== undefined) return null;
  const loc = n.sourceCodeLocation?.startTag;
  const out = { type: 'element', id: next++, tag: n.tagName, attrs, src: `${file}:${loc?.startLine ?? 1}:${loc?.startCol ?? 1}`, children: [] };
  if (n.namespaceURI?.includes('svg')) out.ns = 'svg';
  if (get('data-include')) {
    const rel = get('data-include');
    const fragment = parseFragment(fs.readFileSync(path.join(root, rel), 'utf8'), { sourceCodeLocationInfo: true });
    out.attrs = attrs.filter(a => a[0] !== 'data-include');
    out.children = fragment.childNodes.map(x => convert(x, rel)).filter(Boolean);
  } else out.children = (n.content?.childNodes ?? n.childNodes ?? []).map(x => convert(x, file)).filter(Boolean);
  if (n.tagName === 'link' && get('rel') === 'stylesheet') {
    const file = get('href');
    sheets.push({ kind: 'link', node: out.id, href: file, file, src: out.src, ...parseCss(fs.readFileSync(path.join(root, file), 'utf8'), file) });
  }
  return out;
}
const parsed = parse(fs.readFileSync(path.join(root, 'index.html'), 'utf8'), { sourceCodeLocationInfo: true });
const dom = convert(parsed.childNodes.find(n => n.tagName === 'html'), 'index.html');
process.stdout.write(JSON.stringify({ v: 1, preview: false, scan: { version: 1, state: 'economy', entry: 'index.html', viewport: [1366,768], compat: 'CSS1Compat', title: 'Engraved', dom, sheets } }));
