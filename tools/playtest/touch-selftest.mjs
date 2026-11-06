// The page's phone support (P18 touch) without the game, in headless Chromium:
//   node tools/playtest/touch-selftest.mjs [--www apps/web/www]
// Serves the www dir and opens index.html (the wasm module is not built; the scripts under test do not need it) as
//  - a phone (Pixel 7 emulation: touch, coarse pointer): the web app manifest is linked and valid (display fullscreen,
//    icons that load), the full screen button shows and its tap calls requestFullscreen, and hides once the page is
//    full screen;
//  - a desktop mouse: no button; the game cursor moves to a page element in the edge zone (and back out of it), which
//    is the fix for the cursor that turned into the system arrow in the bottom 20 px.
// Exit code 0 only when every check passed. Needs Playwright (`npm ci` here).
import { createReadStream, existsSync, statSync } from "node:fs";
import { createServer } from "node:http";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { chromium, devices } from "playwright";

const here = path.dirname(fileURLToPath(import.meta.url));
const args = process.argv.slice(2);
const wi = args.indexOf("--www");
const www = path.resolve(wi >= 0 ? args[wi + 1] : path.join(here, "..", "..", "apps", "web", "www"));
const TYPES = {
  ".html": "text/html",
  ".js": "text/javascript",
  ".css": "text/css",
  ".png": "image/png",
  ".webmanifest": "application/manifest+json",
};

const server = createServer((req, res) => {
  const rel = decodeURIComponent(new URL(req.url, "http://x").pathname).replace(/^\/+/, "") || "index.html";
  const file = path.join(www, rel);
  if (!file.startsWith(www) || !existsSync(file) || !statSync(file).isFile()) {
    res.writeHead(404).end();
    return;
  }
  res.writeHead(200, { "content-type": TYPES[path.extname(file)] ?? "application/octet-stream" });
  createReadStream(file).pipe(res);
});
await new Promise((r) => server.listen(0, "127.0.0.1", r));
const base = `http://127.0.0.1:${server.address().port}`;

let failed = 0;
const check = (ok, what, extra = "") => {
  console.log(`${ok ? "ok  " : "FAIL"} ${what}${ok ? "" : ` ${extra}`}`);
  if (!ok) failed++;
};

const browser = await chromium.launch();
try {
  // ---------------------------------------------------------------- the phone
  {
    const ctx = await browser.newContext({ ...devices["Pixel 7"] });
    await ctx.addInitScript(() => {
      window.__fsCalls = [];
      Element.prototype.requestFullscreen = function (o) {
        window.__fsCalls.push(o ?? null);
        Object.defineProperty(document, "fullscreenElement", { configurable: true, get: () => document.documentElement });
        document.dispatchEvent(new Event("fullscreenchange"));
        return Promise.resolve();
      };
    });
    const page = await ctx.newPage();
    await page.goto(`${base}/index.html`);
    await page.waitForTimeout(300);

    const href = await page.getAttribute('link[rel="manifest"]', "href");
    check(href === "./manifest.webmanifest", "the page links the manifest", String(href));
    const man = await (await page.request.get(new URL(href, `${base}/index.html`).href)).json();
    check(man.display === "fullscreen", "manifest display is fullscreen", man.display);
    check(man.display_override?.includes("standalone"), "manifest falls back to standalone");
    check(man.start_url && man.scope, "manifest has start_url and scope");
    let iconsOk = man.icons?.some((i) => i.sizes === "192x192") && man.icons?.some((i) => i.sizes === "512x512");
    for (const i of man.icons ?? []) iconsOk &&= (await page.request.get(`${base}/${i.src}`)).ok();
    check(iconsOk, "manifest icons (192, 512) load");
    const meta = await page.evaluate(() => ({
      cover: /viewport-fit=cover/.test(document.querySelector('meta[name="viewport"]').content),
      apple: document.querySelector('meta[name="apple-mobile-web-app-capable"]')?.content,
    }));
    check(meta.cover && meta.apple === "yes", "viewport-fit=cover and the iOS standalone tags", JSON.stringify(meta));

    const btn = page.locator("#civ5-fs-button");
    check(await btn.isVisible(), "the full screen button shows on a phone");
    await btn.tap();
    await page.waitForTimeout(200);
    const calls = await page.evaluate(() => window.__fsCalls);
    check(calls.length === 1 && calls[0]?.navigationUI === "hide", "a tap on it requests full screen", JSON.stringify(calls));
    check(!(await btn.isVisible()), "the button hides once the page is full screen");
    check(
      (await page.evaluate(() => localStorage.getItem("civ5.fullscreen"))) === "1",
      "the choice is remembered",
    );
    // The touch long press is the client's (touch_press.rs, unit tests); here only that the page does not get in its
    // way: no text selection / callout styles missing and the browser's own menu is not wanted.
    const css = await page.evaluate(() => {
      const c = getComputedStyle(document.body);
      return { sel: c.userSelect, touch: getComputedStyle(document.getElementById("civ5-canvas")).touchAction };
    });
    check(css.sel === "none" && css.touch === "none", "no selection on a long press, the canvas takes the gestures", JSON.stringify(css));
    await ctx.close();
  }

  // ---------------------------------------------------------------- the desktop mouse
  {
    const ctx = await browser.newContext({ viewport: { width: 1000, height: 700 } });
    const page = await ctx.newPage();
    await page.goto(`${base}/index.html`);
    await page.waitForTimeout(300);
    check(!(await page.locator("#civ5-fs-button").count()), "no full screen button for a mouse");

    // The unit: the value parser the cursor swap relies on.
    const parsed = await page.evaluate(async () => {
      const m = await import("./cursor-edge.js");
      return {
        a: m.parseCursor("url(\"blob:http://x/abc\") 4 2, auto"),
        b: m.parseCursor("url(blob:http://x/abc) 17 15, auto"),
        c: m.parseCursor("pointer"),
        zone: [m.inEdgeZone(500, 300, 1000, 700), m.inEdgeZone(500, 690, 1000, 700), m.inEdgeZone(500, 640, 1000, 700), m.inEdgeZone(5, 300, 1000, 700), m.inEdgeZone(995, 300, 1000, 700)],
        lock: [m.pointerLocked({ pointerLockElement: {} }), m.pointerLocked({ pointerLockElement: null }), m.pointerLocked(null)],
      };
    });
    check(parsed.a?.url === "blob:http://x/abc" && parsed.a.x === 4 && parsed.a.y === 2, "parses a quoted cursor url and hot spot", JSON.stringify(parsed.a));
    check(parsed.b?.url === "blob:http://x/abc" && parsed.b.x === 17 && parsed.b.y === 15, "parses an unquoted one", JSON.stringify(parsed.b));
    check(parsed.c === null, "a keyword cursor is not a picture");
    check(JSON.stringify(parsed.zone) === "[false,true,false,true,true]", "the edge zone", JSON.stringify(parsed.zone));
    check(JSON.stringify(parsed.lock) === "[true,false,false]", "a pointer lock hides the soft cursor", JSON.stringify(parsed.lock));

    // winit sets the cursor on the canvas; make one with a real picture.
    const png = (await (await page.request.get(`${base}/icons/icon-192.png`)).body()).toString("base64");
    await page.evaluate((b64) => {
      document.getElementById("civ5-canvas").style.cursor = `url("data:image/png;base64,${b64}") 4 2, auto`;
    }, png);
    const state = () =>
      page.evaluate(() => {
        const el = document.getElementById("civ5-soft-cursor");
        const cs = getComputedStyle(document.getElementById("civ5-canvas")).cursor;
        return { shown: getComputedStyle(el).display !== "none", transform: el.style.transform, nativeHidden: cs === "none" };
      });
    await page.mouse.move(500, 300);
    let st = await state();
    check(!st.shown && !st.nativeHidden, "in the middle the native cursor shows", JSON.stringify(st));
    await page.mouse.move(500, 690);
    await page.waitForTimeout(100);
    st = await state();
    check(st.shown && st.nativeHidden, "in the bottom 20 px the page draws the cursor and the system one is hidden", JSON.stringify(st));
    check(st.transform.replace(/ /g, "") === "translate(496px,688px)", "at the pointer minus the hot spot", st.transform);
    await page.mouse.move(500, 650);
    st = await state();
    check(!st.shown && !st.nativeHidden, "back in the middle the native cursor returns", JSON.stringify(st));
    await ctx.close();
  }
} finally {
  await browser.close();
  server.close();
}
console.log(failed ? `${failed} check(s) failed` : "all checks passed");
process.exit(failed ? 1 : 0);
