// `golden` (the golden-image check of the web client): renders one frame of each golden scene of `--golden-dir`
// (`mapbasis.wv3`, `world.wv3`, `catalog.wv3`, one `<capture>.golden` per capture) in the client and saves its pixels
// as raw RGBA8 and a mask: `<out>/<capture>-<backend>.rgba`, `.mask.r8`, and `.txt` (`W H`). The scene's files are
// served by a static server here: the page is on another origin, so the server sends `Access-Control-Allow-Origin: *`.
import { createServer } from "node:http";
import { existsSync, mkdirSync, readFileSync, readdirSync, statSync, writeFileSync } from "node:fs";
import path from "node:path";
import { openPage, appUrl } from "../lib/browser.mjs";

const CAPTURE_TIMEOUT_MS = 180000;
const BACKENDS = ["webgpu", "webgl2"];

/** A static server of `dir` on 127.0.0.1 (a random port); returns { url, stop() }. */
async function startStatic(dir) {
  const srv = createServer((req, res) => {
    const name = decodeURIComponent(new URL(req.url, "http://x").pathname).replace(/^\/+/, "");
    const file = path.resolve(dir, name);
    const cors = { "Access-Control-Allow-Origin": "*" };
    if (!file.startsWith(path.resolve(dir) + path.sep) || !existsSync(file) || !statSync(file).isFile()) {
      res.writeHead(404, cors).end();
      return;
    }
    res.writeHead(200, { ...cors, "Content-Type": "application/octet-stream" });
    res.end(readFileSync(file));
  });
  await new Promise((r) => srv.listen(0, "127.0.0.1", r));
  return {
    url: `http://127.0.0.1:${srv.address().port}`,
    stop() {
      srv.closeAllConnections();
      return new Promise((r) => srv.close(r));
    },
  };
}

/** `<capture>.golden`: the first line `civ5-golden/1`, then `key values` lines. */
function parseGolden(text, name) {
  const lines = text.split(/\r?\n/).map((l) => l.trim()).filter(Boolean);
  if (lines[0] !== "civ5-golden/1") throw new Error(`${name}.golden: not a civ5-golden/1 file`);
  const kv = {};
  for (const l of lines.slice(1)) {
    const [k, ...rest] = l.split(/\s+/);
    kv[k] = rest.join(" ");
  }
  const [width, height] = (kv.size ?? "").split(/\s+/).map(Number);
  if (!(width > 0 && height > 0)) throw new Error(`${name}.golden: no size line`);
  return {
    capture: kv.capture ?? name,
    width,
    height,
    preset: kv.preset || null,
    disable: (kv.disable ?? "").split(",").map((x) => x.trim()).filter(Boolean),
  };
}

export async function run(ctx) {
  const started = Date.now();
  const o = ctx.opts;
  const result = { scenario: "golden", pass: false, logs: o.logDir, checks: [], numbers: { captures: [] }, consoleErrors: [] };
  const check = (name, pass, detail) => result.checks.push({ name, pass: !!pass, detail: detail ?? "" });
  if (!o.goldenDir) throw new Error("--golden-dir <scene folder> is required for the golden scenario");
  const backend = o.goldenBackend ?? "webgpu";
  if (!BACKENDS.includes(backend)) throw new Error(`--golden-backend ${backend}: want ${BACKENDS.join(" or ")}`);
  const dir = path.resolve(o.goldenDir);
  const out = path.resolve(o.goldenOut ?? o.logDir);
  mkdirSync(out, { recursive: true });
  // The captures: the named ones, else every `*.golden` of the folder.
  const names = o.goldenCaptures
    ? o.goldenCaptures.split(",").map((x) => x.trim()).filter(Boolean)
    : readdirSync(dir).filter((f) => f.endsWith(".golden")).map((f) => f.slice(0, -".golden".length));
  const srv = await startStatic(dir);
  try {
    for (const name of names) {
      const file = path.join(dir, `${name}.golden`);
      if (!existsSync(file)) {
        check(`capture ${name}`, false, `no ${name}.golden in ${dir}`);
        continue;
      }
      const spec = parseGolden(readFileSync(file, "utf8"), name);
      const cap = `${spec.capture} (${backend})`;
      const b = await openPage({
        headed: o.headed,
        // The goldens' pixels are SwiftShader's when headless (stable across machines).
        swiftshader: true,
        logDir: o.logDir,
        consoleFile: `golden-${spec.capture}-${backend}.console.log`,
        viewport: { width: spec.width, height: spec.height },
      });
      try {
        const extra = { golden: srv.url, capture: spec.capture };
        if (spec.preset) extra.preset = spec.preset;
        if (spec.disable.length) extra.disable = spec.disable.join(",");
        await b.page.goto(appUrl(ctx.server.url, { gfx: backend === "webgl2" ? "webgl2" : null }, extra));
        await b.page.waitForFunction(() => !!(window.__civ5test && window.__civ5test.golden), null, {
          timeout: CAPTURE_TIMEOUT_MS,
          polling: 250,
        });
        // The pixels as base64: the bytes go through `String.fromCharCode` in chunks, one `btoa` at the end.
        const g = await b.page.evaluate(() => {
          const g = window.__civ5test.golden;
          if (g.status !== "done") return { status: g.status, error: g.error ?? "" };
          const b64 = (u8) => {
            let bin = "";
            for (let i = 0; i < u8.length; i += 0x8000) bin += String.fromCharCode.apply(null, u8.subarray(i, i + 0x8000));
            return btoa(bin);
          };
          return { status: g.status, width: g.width, height: g.height, rgba: b64(g.rgba), mask: b64(g.mask) };
        });
        if (g.status !== "done") {
          check(`${cap} rendered`, false, g.error || `status ${g.status}`);
        } else {
          const rgba = Buffer.from(g.rgba, "base64");
          const mask = Buffer.from(g.mask, "base64");
          const pixels = spec.width * spec.height;
          const sizeOk = g.width === spec.width && g.height === spec.height && rgba.length === pixels * 4 && mask.length === pixels;
          check(`${cap} size`, sizeOk, `${g.width}x${g.height} (rgba ${rgba.length} B, mask ${mask.length} B), spec ${spec.width}x${spec.height}`);
          if (sizeOk) {
            const base = path.join(out, `${spec.capture}-${backend}`);
            writeFileSync(`${base}.rgba`, rgba);
            writeFileSync(`${base}.mask.r8`, mask);
            writeFileSync(`${base}.txt`, `${g.width} ${g.height}\n`);
            result.numbers.captures.push({ capture: spec.capture, backend, width: g.width, height: g.height, file: `${base}.rgba` });
          }
        }
      } catch (e) {
        check(`${cap} rendered`, false, e.message.split("\n")[0]);
      }
      // The page's console errors fail the capture, as in `smoke` (none are allowed on the golden page).
      check(`${cap} no console errors`, b.errors.length === 0, `${b.errors.length} error(s)${b.errors.length ? `: ${b.errors[0]}` : ""}`);
      result.consoleErrors.push(...b.errors);
      await b.close();
    }
  } finally {
    await srv.stop();
  }
  result.pass = result.checks.length > 0 && result.checks.every((c) => c.pass);
  result.durationMs = Date.now() - started;
  return result;
}
