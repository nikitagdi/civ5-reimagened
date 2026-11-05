// Runs the replica self-test (case `replica_load`) in headless Chromium:
//   node tools/web-selftest/run.mjs <bundle-dir> --fixture <dir> [--scenarios tiny,small] [--timeout-ms N]
// Serves the bundle (built with the feature `replica-selftest`) and the fixture dir (`/fixture/<scenario>/...`) on a
// local port, opens replica-selftest.html?fixture=fixture/<scenario> per scenario, prints the console and the
// report, and exits 0 only if every scenario says PASS. Needs Playwright (tools/playtest after `npm ci` there, else
// global) and a Chromium (PLAYWRIGHT_BROWSERS_PATH, or CHROMIUM_PATH for an explicit executable).
import { createReadStream, existsSync, statSync } from "node:fs";
import { createServer } from "node:http";
import { createRequire } from "node:module";
import { execSync } from "node:child_process";
import path from "node:path";

const args = process.argv.slice(2);
function take(flag, dflt) {
  const i = args.indexOf(flag);
  return i >= 0 ? args.splice(i, 2)[1] : dflt;
}
const timeoutMs = Number(take("--timeout-ms", "300000"));
const fixture = path.resolve(take("--fixture", "target/fixture"));
const withMt = args.includes("--mt") ? (args.splice(args.indexOf("--mt"), 1), true) : false; // also run every scenario in pkg-mt
const scenarios = take("--scenarios", "tiny").split(",").filter(Boolean);
const root = path.resolve(args[0] ?? "target/web-selftest");
if (!existsSync(path.join(root, "replica-selftest.html"))) {
  console.error(`no replica-selftest.html in ${root}; run cargo xtask web-selftest`);
  process.exit(2);
}

function loadPlaywright() {
  // The pinned copy of tools/playtest (`npm ci` there) first, then a global one.
  const pinned = new URL("../playtest/package.json", import.meta.url);
  try {
    return createRequire(pinned)("playwright");
  } catch {
    /* not installed there */
  }
  const require = createRequire(import.meta.url);
  try {
    return require("playwright");
  } catch {
    const global = execSync("npm root -g", { encoding: "utf8" }).trim();
    return createRequire(path.join(global, "x.js"))("playwright");
  }
}
const { chromium } = loadPlaywright();

const types = {
  ".html": "text/html",
  ".js": "text/javascript",
  ".mjs": "text/javascript",
  ".wasm": "application/wasm",
  ".json": "application/json",
  ".css": "text/css",
};
function resolveFile(urlPath) {
  const rel = path.normalize(decodeURIComponent(urlPath)).replace(/^([/\\]|\.\.[/\\])+/, "");
  const inFixture = rel.startsWith("fixture" + path.sep);
  const base = inFixture ? fixture : root;
  const file = path.join(base, inFixture ? rel.slice("fixture".length + 1) : rel);
  if (!file.startsWith(base)) return null;
  return existsSync(file) && statSync(file).isFile() ? file : null;
}
const server = createServer((req, res) => {
  const file = resolveFile(new URL(req.url, "http://x").pathname);
  if (!file) {
    res.writeHead(404).end();
    return;
  }
  // COOP/COEP: the threaded build needs a cross-origin isolated page (shared memory).
  res.writeHead(200, {
    "content-type": types[path.extname(file)] ?? "application/octet-stream",
    "cross-origin-opener-policy": "same-origin",
    "cross-origin-embedder-policy": "require-corp",
    "cross-origin-resource-policy": "same-origin",
  });
  createReadStream(file).pipe(res);
});
await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
const port = server.address().port;

const browser = await chromium.launch({
  executablePath: process.env.CHROMIUM_PATH || undefined,
  args: ["--no-sandbox"],
});
let failed = false;
const runs = scenarios.flatMap((s) => (withMt ? [[s, false], [s, true]] : [[s, false]]));
for (const [scenario, mt] of runs) {
  const label = mt ? `${scenario}/mt` : scenario;
  const page = await browser.newPage();
  const lines = [];
  page.on("console", (m) => {
    lines.push(m.text());
    if (!m.text().startsWith("replica_load:")) console.log(`[console.${m.type()}] ${m.text()}`);
  });
  page.on("pageerror", (e) => console.log(`[pageerror] ${e.message}`));
  const start = Date.now();
  await page.goto(`http://127.0.0.1:${port}/replica-selftest.html?fixture=fixture/${scenario}${mt ? "&mt=1" : ""}`);
  let report = null;
  while (Date.now() - start < timeoutMs) {
    report = lines.find((l) => l.startsWith("replica_load:"));
    if (report) break;
    await new Promise((r) => setTimeout(r, 100));
  }
  await page.close();
  if (!report) {
    console.error(`FAIL ${label}: no replica_load report within the timeout`);
    failed = true;
    continue;
  }
  console.log(`[${label}] ${report}`);
  if (!report.startsWith("replica_load: PASS")) failed = true;
}
await browser.close();
server.close();
console.log(failed ? "RESULT: FAIL" : "RESULT: PASS");
process.exit(failed ? 1 : 0);
