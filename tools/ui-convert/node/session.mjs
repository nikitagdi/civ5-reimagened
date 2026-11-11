// One converter session: the local server, one Chromium page, and the network guard.
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
import { extractPage } from "./extract.mjs";

export class ProtocolError extends Error {
  constructor(code, message) {
    super(message);
    this.code = code;
  }
}

const need = (cond, msg) => { if (!cond) throw new ProtocolError("E9-003", msg); };

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
      for (const k of ["fonts_dir", "profile"]) need(req[k] == null || typeof req[k] === "string", `open: ${k} must be a string`);
      need(req.preview == null || typeof req.preview === "boolean", "open: preview must be a boolean");
      const root = path.resolve(req.skin_dir);
      if (!fs.existsSync(root) || !fs.statSync(root).isDirectory()) throw new ProtocolError("E9-004", `skin directory not found: ${req.skin_dir}`);
      if (!fs.existsSync(path.join(root, entry)) || !fs.statSync(path.join(root, entry)).isFile()) throw new ProtocolError("E9-004", `entry file not found: ${entry}`);

      // The preview script is injected only when asked for and present.
      const preview = req.preview === true && previewAvailable();
      const server = await startServer({ root, port, preview, fontsDir: req.fonts_dir ? path.resolve(req.fonts_dir) : null });
      let browser;
      try {
        browser = await launchChromium();
      } catch (e) {
        await server.close();
        throw new ProtocolError("E9-002", `Chromium did not start: ${String(e && e.message).split("\n")[0]}`);
      }
      const blocked = new Set();
      const context = await browser.newContext({
        viewport: { width: 1920, height: 1080 },
        deviceScaleFactor: 1,
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
      const page = await context.newPage();
      st = { root, entry, server, browser, context, page, blocked };
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
      return extractPage({ page: st.page, server: st.server, entry: st.entry, req, state: st.state || "" });
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
