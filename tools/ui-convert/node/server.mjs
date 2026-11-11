// Local static server for the skin directory (127.0.0.1 only).
//
// HTML files are served with a source position written into every element that has a start tag in
// the file: data-ui2-src="<path>:<line>:<column>" (path relative to the skin root, "/" separators,
// 1-based line and column of the "<"). The attribute is spliced into the original text, so nothing else
// in the document changes (whitespace, entities and NBSP stay as written).
//
// Fonts: when `fontsDir` is given, a request whose path exists below it is served from there first
// (the converter serves subset fonts this way). `preview: true` adds the preview script to every page.
import http from "node:http";
import fs from "node:fs";
import path from "node:path";
import { parse } from "parse5";

const MIME = {
  ".html": "text/html; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".mjs": "text/javascript; charset=utf-8",
  ".json": "application/json; charset=utf-8",
  ".svg": "image/svg+xml",
  ".png": "image/png",
  ".webp": "image/webp",
  ".jpg": "image/jpeg",
  ".jpeg": "image/jpeg",
  ".ttf": "font/ttf",
  ".otf": "font/otf",
  ".woff": "font/woff",
  ".woff2": "font/woff2",
  ".toml": "text/plain; charset=utf-8",
};

const PREVIEW_URL = "/__ui2/preview.js";
const PREVIEW_FILE = new URL("preview/preview.js", import.meta.url);

export function escapeAttr(s) {
  return s.replace(/&/g, "&amp;").replace(/"/g, "&quot;");
}

/**
 * Adds data-ui2-src to every element of an HTML source and collects the inline <style> blocks.
 * Returns { html, styles } where styles = [{ src, line, col, text }] (position of the text start).
 */
export function annotateHtml(source, relPath, { preview = false } = {}) {
  const doc = parse(source, { sourceCodeLocationInfo: true });
  const inserts = []; // [offset, text]
  const styles = [];
  let headEnd = -1;

  const visit = (node) => {
    if (node.nodeName && node.nodeName[0] !== "#") {
      const loc = node.sourceCodeLocation;
      const st = loc && loc.startTag;
      if (st) {
        const m = /^<([^\s/>]+)/.exec(source.slice(st.startOffset, st.startOffset + 256));
        const src = `${relPath}:${st.startLine}:${st.startCol}`;
        inserts.push([st.startOffset + m[0].length, ` data-ui2-src="${escapeAttr(src)}"`]);
        if (node.nodeName === "style") {
          const text = node.childNodes.find((c) => c.nodeName === "#text");
          if (text && text.sourceCodeLocation) {
            const tl = text.sourceCodeLocation;
            styles.push({ src, line: tl.startLine, col: tl.startCol, text: source.slice(tl.startOffset, tl.endOffset) });
          }
        }
        if (node.nodeName === "head" && loc.endTag) headEnd = loc.endTag.startOffset;
      }
    }
    for (const c of node.childNodes || []) visit(c);
    if (node.content) visit(node.content); // <template> contents
  };
  visit(doc);

  if (preview) {
    const tag = `<script src="${PREVIEW_URL}"></script>`;
    inserts.push([headEnd >= 0 ? headEnd : source.length, tag]);
  }
  // Splice from the end so earlier offsets stay valid; stable for equal offsets.
  let html = source;
  inserts.map((x, i) => [x[0], i, x[1]]).sort((a, b) => b[0] - a[0] || b[1] - a[1])
    .forEach(([off, , text]) => { html = html.slice(0, off) + text + html.slice(off); });
  return { html, styles };
}

/**
 * Starts the server. Resolves to { port, url, close(), requests(), styleBlocks(relPath), resolveFile(rel),
 * reset() }.
 */
export async function startServer({ root, port = 0, fontsDir = null, preview = false }) {
  const rootDir = path.resolve(root);
  const log = new Map(); // url path -> status of the last response
  const htmlCache = new Map(); // relPath -> { mtimeMs, html, styles }

  function resolveFile(rel) {
    // rel: URL path without the leading slash, already percent-decoded
    const full = path.resolve(rootDir, rel);
    const inside = full === rootDir || full.startsWith(rootDir + path.sep);
    return inside ? full : null;
  }

  function loadHtml(rel, full) {
    const st = fs.statSync(full);
    const hit = htmlCache.get(rel);
    if (hit && hit.mtimeMs === st.mtimeMs && hit.preview === preview) return hit;
    const source = fs.readFileSync(full, "utf8");
    const out = { ...annotateHtml(source, rel, { preview }), mtimeMs: st.mtimeMs, preview };
    htmlCache.set(rel, out);
    return out;
  }

  const server = http.createServer((req, res) => {
    let urlPath = "/";
    const done = (status, body, type) => {
      log.set(urlPath, status);
      res.writeHead(status, {
        "Content-Type": type || "text/plain; charset=utf-8",
        "Cache-Control": "no-store",
        "X-Content-Type-Options": "nosniff",
      });
      res.end(req.method === "HEAD" ? undefined : body);
    };
    try {
      if (req.method !== "GET" && req.method !== "HEAD") return done(405, "method not allowed");
      // Reject foreign Host headers (DNS rebinding): only the loopback address is ours.
      const host = (req.headers.host || "").replace(/:\d+$/, "");
      if (host !== "127.0.0.1") return done(403, "forbidden host");
      const u = new URL(req.url, "http://127.0.0.1");
      urlPath = u.pathname;
      let rel;
      try { rel = decodeURIComponent(urlPath.slice(1)); } catch { return done(400, "bad path"); }
      if (rel.includes("\0") || rel.split(/[\\/]/).includes("..")) return done(403, "forbidden");

      if (urlPath === PREVIEW_URL) {
        if (!fs.existsSync(PREVIEW_FILE)) return done(404, "preview script is not available");
        return done(200, fs.readFileSync(PREVIEW_FILE), MIME[".js"]);
      }
      if (rel.startsWith("__ui2/")) return done(404, "not found");

      let full = null;
      if (fontsDir) {
        const f = path.resolve(fontsDir, rel);
        if (f.startsWith(path.resolve(fontsDir) + path.sep) && fs.existsSync(f) && fs.statSync(f).isFile()) full = f;
      }
      if (!full) {
        full = resolveFile(rel);
        if (!full || !fs.existsSync(full) || !fs.statSync(full).isFile()) return done(404, "not found");
      }
      const ext = path.extname(full).toLowerCase();
      if (ext === ".html") {
        const relHtml = path.relative(rootDir, full).split(path.sep).join("/");
        return done(200, loadHtml(relHtml, full).html, MIME[".html"]);
      }
      return done(200, fs.readFileSync(full), MIME[ext] || "application/octet-stream");
    } catch (e) {
      return done(500, String(e && e.message));
    }
  });

  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(port, "127.0.0.1", resolve);
  });
  const actualPort = server.address().port;

  return {
    port: actualPort,
    url: `http://127.0.0.1:${actualPort}`,
    root: rootDir,
    resolveFile,
    /** Inline <style> blocks of an HTML file (relative path), as annotated when it was served. */
    styleBlocks(rel) {
      const full = resolveFile(rel);
      return full && fs.existsSync(full) ? loadHtml(rel, full).styles : [];
    },
    /** [{ path, status }] of all requests since the last reset(), sorted by path. */
    requests() {
      return [...log.entries()].sort((a, b) => (a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : 0)).map(([p, status]) => ({ path: p, status }));
    },
    reset() { log.clear(); },
    invalidate() { htmlCache.clear(); },
    close() {
      return new Promise((resolve) => { server.close(() => resolve()); server.closeAllConnections?.(); });
    },
  };
}
