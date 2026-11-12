// One converter session: the local server, one Chromium page (and the helper pages of the style extraction),
// and the network guard.
//
// Network rule: the page may talk only to the local server of this session. Everything else (a fonts
// CDN, another loopback port, WebSockets elsewhere) is aborted before it leaves the machine and is
// reported as error E9-001 with the URL.
//
// Error codes raised here (category E9, environment):
//   E9-001 request outside the local server (reported in the scan IR, not thrown)
//   E9-002 Chromium could not be started
//   E9-003 bad request (unknown or unimplemented command, bad or missing field, wrong session state)
//   E9-004 skin directory or entry file not found
import fs from "node:fs";
import path from "node:path";
import { launchChromium, chromiumFlags } from "./chromium.mjs";
import { startServer, previewAvailable } from "./server.mjs";
import { scanPage } from "./ir.mjs";
import { extractPage, extractWorkers } from "./extract.mjs";
import { rasterItems } from "./raster.mjs";
import { rectsOf } from "./rects.mjs";

export class ProtocolError extends Error {
  constructor(code, message) {
    super(message);
    this.code = code;
  }
}

const need = (cond, msg) => { if (!cond) throw new ProtocolError("E9-003", msg); };

/**
 * A browser context whose network is limited to the local server (everything else is aborted and recorded in
 * `blocked`, E9-001).
 */
async function newGuardedContext(browser, server, blocked, viewport, dsf) {
  const context = await browser.newContext({
    viewport,
    deviceScaleFactor: dsf,
    serviceWorkers: "block",
    acceptDownloads: false,
  });
  const ours = (url) => {
    try { return new URL(url).origin === server.url; } catch { return false; }
  };
  await context.route("**/*", (route) => {
    const url = route.request().url();
    if (ours(url) || /^(data|blob|about):/i.test(url)) return route.continue();
    blocked.add(url);
    return route.abort("blockedbyclient");
  });
  if (context.routeWebSocket) {
    await context.routeWebSocket(/.*/, (ws) => {
      const url = ws.url();
      if (ours(url.replace(/^ws/, "http"))) { ws.connectToServer(); return; }
      blocked.add(url);
      ws.close();
    });
  }
  return context;
}

export function createSession() {
  let st = null;

  return {
    async open(req) {
      need(!st, "a session is already open");
      need(typeof req.skin_dir === "string" && req.skin_dir !== "", "open: skin_dir (string) is required");
      const entry = req.entry ?? "index.html";
      need(typeof entry === "string", "open: entry must be a string");
      const port = req.port ?? 0;
      need(Number.isInteger(port) && port >= 0 && port < 65536, "open: port must be an integer 0..65535");
      for (const k of ["fonts_dir", "profile", "pkg_dir"]) need(req[k] == null || typeof req[k] === "string", `open: ${k} must be a string`);
      need(req.preview == null || typeof req.preview === "boolean", "open: preview must be a boolean");
      need(req.list_keys == null || (typeof req.list_keys === "object" && !Array.isArray(req.list_keys)), "open: list_keys must be an object");
      const virtualFiles = req.virtual ?? {};
      need(typeof virtualFiles === "object" && !Array.isArray(virtualFiles) && Object.entries(virtualFiles).every(([k, v]) =>
        typeof v === "string" && k !== "" && !k.startsWith("/") && !k.includes("\\") && !k.split("/").includes("..")), "open: virtual maps relative paths to texts");
      const root = path.resolve(req.skin_dir);
      if (!fs.existsSync(root) || !fs.statSync(root).isDirectory()) throw new ProtocolError("E9-004", `skin directory not found: ${req.skin_dir}`);
      const entryFile = path.join(root, entry);
      if (!Object.hasOwn(virtualFiles, entry) && (!fs.existsSync(entryFile) || !fs.statSync(entryFile).isFile())) throw new ProtocolError("E9-004", `entry file not found: ${entry}`);

      // The preview script is injected only when asked for and present.
      const preview = req.preview === true && previewAvailable();
      const server = await startServer({
        root,
        port,
        preview,
        fontsDir: req.fonts_dir ? path.resolve(req.fonts_dir) : null,
        pkgDir: req.pkg_dir ? path.resolve(req.pkg_dir) : null,
        listKeys: req.list_keys ?? null,
        virtualFiles,
      });
      let browser;
      try {
        browser = await launchChromium();
      } catch (e) {
        await server.close();
        throw new ProtocolError("E9-002", `Chromium did not start: ${String(e && e.message).split("\n")[0]}`);
      }
      const blocked = new Set();
      const guarded = (viewport, dsf) => newGuardedContext(browser, server, blocked, viewport, dsf);
      const context = await guarded({ width: 1920, height: 1080 }, 1);
      const page = await context.newPage();
      st = { root, entry, server, browser, context, page, helpers: [], blocked, guarded, skin: path.basename(root) };
      return { chrome: browser.version(), flags: chromiumFlags(), base_url: `${server.url}/`, preview };
    },

    async scan(req = {}) {
      need(st, "scan: no open session");
      const state = req.state ?? "";
      need(typeof state === "string", "scan: state must be a string");
      const viewport = req.viewport ?? [1920, 1080];
      need(Array.isArray(viewport) && viewport.length === 2 && viewport.every((n) => Number.isInteger(n) && n > 0), "scan: viewport must be [w, h]");
      st.state = state;
      return scanPage({ page: st.page, server: st.server, entry: st.entry, state, viewport, blocked: st.blocked });
    },

    /** Answers the converter's style plan (see extract.mjs) on the page of the last scan. */
    async extract(req = {}) {
      need(st, "extract: no open session");
      // The helper pages stay open for the next cycle of watch mode.
      while (st.helpers.length < extractWorkers() - 1) st.helpers.push(await st.context.newPage());
      const pages = [st.page, ...st.helpers];
      const out = await extractPage({ pages, server: st.server, entry: st.entry, req, state: st.state || "" });
      // Computed `url()` values carry the server's port, which changes per run: the answer names the server as
      // port 0, so stored answers stay the same (the converter maps such URLs back to skin paths).
      return JSON.parse(JSON.stringify(out).replaceAll(`${st.server.url}/`, "http://127.0.0.1:0/"));
    },

    /** Chrome's rectangle dump of the page in layout mode (see rects.mjs). */
    async rects(req = {}) {
      need(st, "rects: no open session");
      const api = {
        newContext: (viewport, dsf) => st.guarded(viewport, dsf),
        server: () => st.server,
        entry: () => st.entry,
      };
      return rectsOf({ session: api, req, skin: req.skin ?? st.skin });
    },

    /** Rasterizes pictures at device scale 1 and 2 (see raster.mjs). */
    async raster(req = {}) {
      need(st, "raster: no open session");
      return rasterItems({ context: st.context, server: st.server, req });
    },

    /** The next scan always loads the page again; this drops what the server cached (file changes). */
    async reload() {
      need(st, "reload: no open session");
      st.server.invalidate();
      return {};
    },

    async close() {
      if (!st) return {};
      const s = st;
      st = null;
      await s.browser.close().catch(() => {});
      await s.server.close();
      return {};
    },
  };
}
