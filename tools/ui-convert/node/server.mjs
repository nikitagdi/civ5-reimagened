// Local static server for the skin directory (127.0.0.1 only).
//
// HTML files are served with a source position written into every element that has a start tag in
// the file: data-ui2-src="<path>:<line>:<column>" (path relative to the skin root, "/" separators,
// 1-based line and column of the "<"). The attribute is spliced into the original text, so nothing else
// in the document changes (whitespace, entities and NBSP stay as written).
//
// Fonts: when `fontsDir` is given, a request whose path exists below it is served from there first
// (the converter serves subset fonts this way). `preview: true` adds the preview script to every page.
//
// List keys: `listKeys` (model path of a list -> its key field and kind, from the converter) is prepended to the
// preview script as `window.__ui2keys`, so the preview keys loop instances as the runtime does.
//
// Skin state: the text of the folder's `skin.toml` (when there is one) is prepended to the preview script as
// `window.__ui2skin = { toml }`: the preview mounts its `[state]` defaults under the sample's state.
//
// Virtual files: `virtualFiles` (relative path -> text) are served as if they were files of the folder, before the
// folder's own files; the converter's fragment mode puts the page around a mod's panels there.
//
// Packages (watch mode): when `pkgDir` is given, `GET /pkg/<id>.ui2`, `/pkg/<id>.rev` and
// `/pkg/<id>.errors.json` are files of that folder (the converter writes them; `.rev` grows with every
// package that built). These answers carry `Access-Control-Allow-Origin: *` and
// `Cross-Origin-Resource-Policy: cross-origin`: the game page that polls them is cross-origin isolated.
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
export const PREVIEW_FILE = new URL("preview/preview.js", import.meta.url);

/** Whether the preview script exists (it expands includes, loops and bindings in the page). */
export function previewAvailable() {
  return fs.existsSync(PREVIEW_FILE);
}

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
/** The content type of a package file name, or null when the name is not one (`<id>.ui2|rev|errors.json`). */
export function pkgType(name) {
  if (!/^[a-z][a-z0-9-]*\.(ui2|rev|errors\.json)$/.test(name)) return null;
  if (name.endsWith(".ui2")) return "application/octet-stream";
  if (name.endsWith(".rev")) return "text/plain; charset=utf-8";
  return MIME[".json"];
}

export async function startServer({ root, port = 0, fontsDir = null, preview = false, pkgDir = null, listKeys = null, virtualFiles = null }) {
  const rootDir = path.resolve(root);
  const log = new Map(); // url path -> status of the last response
  const htmlCache = new Map(); // relPath -> { mtimeMs, html, styles }
  // Files the converter makes up (the page around the panels of a mod): relative path -> text. They are served
  // before the folder's own files and are annotated like them.
  const virtual = new Map(Object.entries(virtualFiles || {}));

  function resolveFile(rel) {
    // rel: URL path without the leading slash, already percent-decoded
    const full = path.resolve(rootDir, rel);
    const inside = full === rootDir || full.startsWith(rootDir + path.sep);
    return inside ? full : null;
  }

  /** The text of a file of the folder (a virtual file first), or null. */
  function readText(rel) {
    if (virtual.has(rel)) return virtual.get(rel);
    const full = resolveFile(rel);
    return full && fs.existsSync(full) && fs.statSync(full).isFile() ? fs.readFileSync(full, "utf8") : null;
  }

  function loadHtml(rel, full) {
    if (virtual.has(rel)) {
      const hit = htmlCache.get(rel);
      if (hit && hit.preview === preview) return hit;
      const out = { ...annotateHtml(virtual.get(rel), rel, { preview }), mtimeMs: 0, preview };
      htmlCache.set(rel, out);
      return out;
    }
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
    let cors = false;
    const done = (status, body, type) => {
      log.set(urlPath, status);
      const headers = {
        "Content-Type": type || "text/plain; charset=utf-8",
        "Cache-Control": "no-store",
        "X-Content-Type-Options": "nosniff",
      };
      if (cors) {
        headers["Access-Control-Allow-Origin"] = "*";
        headers["Cross-Origin-Resource-Policy"] = "cross-origin";
      }
      res.writeHead(status, headers);
      res.end(req.method === "HEAD" ? undefined : body);
    };
    try {
      cors = pkgDir !== null && (req.url || "").startsWith("/pkg/");
      if (cors && req.method === "OPTIONS") {
        res.writeHead(204, {
          "Access-Control-Allow-Origin": "*",
          "Access-Control-Allow-Methods": "GET, HEAD",
          "Cross-Origin-Resource-Policy": "cross-origin",
        });
        return res.end();
      }
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
        // The list keys of the model schema go in front of the script (no request of their own, so the page
        // loads the same files with and without them).
        const keys = listKeys ? `window.__ui2keys = ${JSON.stringify(listKeys)};\n` : "";
        // The skin's skin.toml as it is now (its [state] defaults), read at every load of the script.
        const toml = readText("skin.toml");
        const skin = toml !== null ? `window.__ui2skin = ${JSON.stringify({ toml })};\n` : "";
        return done(200, keys + skin + fs.readFileSync(PREVIEW_FILE, "utf8"), MIME[".js"]);
      }
      if (rel.startsWith("__ui2/")) return done(404, "not found");
      if (virtual.has(rel)) {
        const ext = path.extname(rel).toLowerCase();
        if (ext === ".html") return done(200, loadHtml(rel, null).html, MIME[".html"]);
        return done(200, virtual.get(rel), MIME[ext] || "text/plain; charset=utf-8");
      }
      if (cors) {
        const name = rel.slice("pkg/".length);
        const type = pkgType(name);
        const full = type && path.join(pkgDir, name);
        if (!full || !fs.existsSync(full)) return done(404, "no such package file");
        return done(200, fs.readFileSync(full), type);
      }

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
    readText,
    /** Inline <style> blocks of an HTML file (relative path), as annotated when it was served. */
    styleBlocks(rel) {
      if (virtual.has(rel)) return loadHtml(rel, null).styles;
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
