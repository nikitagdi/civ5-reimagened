// `ui2-reload`: hot reload of a UI v2 skin from its sources (`?ui2dev=`), on the sample mode (`?ui2sample=hud`), so
// no game is needed (the fixture server or `--attach <url>` both serve the page):
//   cargo xtask playtest --scenario ui2-reload
//
// 1. A copy of `skins/` goes to `<logs>/ui2-reload/skins` and `ui2-convert watch` runs on its `engraved` (port 8096,
//    `UI2_RELOAD_PORT`), as `cargo xtask dev --ui-dev engraved` does.
// 2. The page `?ui=v2:engraved&ui2sample=hud&ui2bg=202020&ui2dev=http://127.0.0.1:<port>` shows the HUD of the sample
//    without a game (the top strip's gold is the sample's) and takes the watch server's first build
//    (`ui2().reload.rev` >= 1).
// 3. An edit of `css/hud.css` (wider yields) is on the screen within 5 s of the save without leaving the page: the
//    rectangle of `top.yield.gold` (`ui2Rects`) changes and `ui2().reload.rev` grows; `appliedAt - savedAt` is reported.
// 4. A broken edit (a property outside the profile) shows the converter's error as a notice (`state().ui.notice`),
//    the running skin stays; the fix brings the next build.
// 5. The page asked the watch server for `/pkg/engraved.*` only and nothing of the game's mods (the mod set and its
//    hash are not on this path).
import { spawn, spawnSync } from "node:child_process";
import { appendFileSync, cpSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { openPage, appUrl } from "../lib/browser.mjs";

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const REPO = fileURLToPath(new URL("../../../", import.meta.url));
const PORT = Number(process.env.UI2_RELOAD_PORT ?? 8096);
/** How long the first build of the watch (a full cycle with Chromium) may take. */
const FIRST_BUILD_MS = Number(process.env.UI2_RELOAD_FIRST_MS ?? 240000);

async function ui2(page) {
  return page.evaluate(() => (window.__civ5test?.ui2 ? window.__civ5test.ui2() : null)).catch(() => null);
}

async function goldRect(page) {
  const r = await page.evaluate(() => window.__civ5test.ui2Rects(["top.yield.gold"])).catch(() => null);
  return Array.isArray(r) && r.length ? r[0] : null;
}

/** Polls `pred(probe)` every 100 ms for `ms`; `{ok, probe, ms}`. */
async function waitUi(page, pred, ms) {
  const t0 = Date.now();
  let probe = null;
  for (;;) {
    probe = await ui2(page);
    if (probe && pred(probe)) return { ok: true, probe, ms: Date.now() - t0 };
    if (Date.now() - t0 > ms) return { ok: false, probe, ms: Date.now() - t0 };
    await sleep(100);
  }
}

/** `ui2-convert watch <dir> --port <port>`; resolves with the process once the first cycle printed its line. */
function startWatch(dir, logFile) {
  const child = spawn("cargo", ["run", "-q", "-p", "ui2-convert", "--", "watch", dir, "--port", String(PORT)], {
    cwd: REPO,
    stdio: ["ignore", "pipe", "pipe"],
    shell: process.platform === "win32",
  });
  const lines = [];
  const take = (b) => {
    const text = String(b);
    appendFileSync(logFile, text);
    for (const l of text.split(/\r?\n/)) if (l.trim()) lines.push(l);
  };
  child.stdout.on("data", take);
  child.stderr.on("data", take);
  return { child, lines };
}

function stopWatch(w) {
  if (!w?.child || w.child.exitCode !== null) return;
  if (process.platform === "win32") {
    // The whole tree (the shell, cargo, the converter and its Chromium), and before the run ends.
    spawnSync("taskkill", ["/pid", String(w.child.pid), "/T", "/F"], { stdio: "ignore" });
  } else {
    w.child.kill("SIGTERM");
  }
}

export async function run(ctx) {
  const o = ctx.opts;
  const started = Date.now();
  const result = { scenario: "ui2-reload", pass: false, logs: o.logDir, checks: [], numbers: {}, consoleErrors: [] };
  const check = (name, pass, detail) => result.checks.push({ name, pass: !!pass, detail: detail ?? "" });
  const base = o.attach ?? ctx.server?.url;
  const work = path.join(o.logDir, "ui2-reload");
  const skins = path.join(work, "skins");
  const skin = path.join(skins, "engraved");
  const css = path.join(skin, "css", "hud.css");
  let watch = null;
  let b = null;
  let error = null;
  try {
    rmSync(work, { recursive: true, force: true });
    mkdirSync(work, { recursive: true });
    cpSync(path.join(REPO, "skins"), skins, { recursive: true });
    const cssText = readFileSync(css, "utf8");
    watch = startWatch(skin, path.join(o.logDir, "ui2-watch.log"));
    const tWatch = Date.now();
    for (;;) {
      if (watch.lines.some((l) => /ui2\.watch cycle_ms=.*rev=[1-9]/.test(l))) break;
      if (watch.child.exitCode !== null) throw new Error(`ui2-convert watch exited: ${watch.lines.slice(-5).join(" | ")}`);
      if (Date.now() - tWatch > FIRST_BUILD_MS) throw new Error(`no first build of the watch in ${FIRST_BUILD_MS / 1000} s`);
      await sleep(250);
    }
    result.numbers.firstBuildMs = Date.now() - tWatch;
    const dev = `http://127.0.0.1:${PORT}`;

    b = await openPage({ headed: o.headed, logDir: o.logDir, viewport: { width: 1600, height: 900 }, consoleFile: "console-reload.log" });
    const requests = [];
    b.page.on("request", (r) => {
      if (r.url().startsWith(dev)) requests.push(new URL(r.url()).pathname);
    });
    await b.page.goto(appUrl(base, o, { ui: "v2:engraved", ui2sample: "hud", ui2bg: "202020", ui2dev: dev, stats: "1" }));
    const sample = JSON.parse(readFileSync(path.join(REPO, "skins/engraved/sample/hud.json"), "utf8"));
    const gold = String(sample.model.top.gold);
    const up = await waitUi(b.page, (p) => p.state === "running" && p.active && p.texts.some((t) => t.part === "top.yield.gold" && t.text === gold), 120000);
    result.numbers.sample = { ms: up.ms, gold: up.probe?.texts?.filter((t) => t.part === "top.yield.gold") };
    check("?ui2sample=hud: the HUD shows the sample's gold without a game", up.ok, JSON.stringify(result.numbers.sample));
    if (!up.ok) throw new Error("the sample HUD did not come up");
    const first = await waitUi(b.page, (p) => (p.reload?.rev ?? 0) >= 1, 10000);
    check("the watch server's first build is taken", first.ok, JSON.stringify(first.probe?.reload));
    const rev0 = first.probe?.reload?.rev ?? 0;
    // The edits are seen once the page polled with the build up.
    await sleep(1000);
    const rect0 = await goldRect(b.page);

    // 3. A CSS edit.
    const timeOrigin = await b.page.evaluate(() => performance.timeOrigin);
    const savedAt = Date.now();
    writeFileSync(css, `${cssText}\n.yield { padding-left: 31px; }\n`);
    let rect1 = null;
    let probe = null;
    for (;;) {
      probe = await ui2(b.page);
      if ((probe?.reload?.rev ?? 0) > rev0) {
        rect1 = await goldRect(b.page);
        if (rect1 && rect0 && (rect1.x !== rect0.x || rect1.w !== rect0.w)) break;
      }
      if (Date.now() - savedAt > 20000) break;
      await sleep(100);
    }
    const wallMs = Date.now() - savedAt;
    const appliedEpoch = probe?.reload ? timeOrigin + probe.reload.appliedAt : null;
    result.numbers.edit = { rev: [rev0, probe?.reload?.rev], rect0, rect1, wallMs, appliedMinusSaved: appliedEpoch && Math.round(appliedEpoch - savedAt), cycles: watch.lines.filter((l) => l.includes("cycle_ms")).slice(-2) };
    check("a CSS edit is on the screen within 5 s, without leaving the page", rect1 && rect0 && (rect1.x !== rect0.x || rect1.w !== rect0.w) && wallMs <= 5000,
      JSON.stringify(result.numbers.edit));
    const st = await b.page.evaluate(() => window.__civ5test.state()).catch(() => null);
    check("the same page: no new load (the sample HUD stays up)", st?.ui?.active === "v2", JSON.stringify(st?.ui));

    // 4. A broken edit: the converter's error is a notice, the skin stays; the fix builds again.
    const rev1 = probe?.reload?.rev ?? rev0;
    writeFileSync(css, `${cssText}\n.yield { padding-left: 31px; float: left; }\n`);
    let notice = null;
    const tErr = Date.now();
    while (Date.now() - tErr < 20000) {
      const s = await b.page.evaluate(() => window.__civ5test.state()).catch(() => null);
      notice = s?.ui?.notice ?? null;
      if (notice && /did not build/.test(notice)) break;
      await sleep(200);
    }
    const still = await ui2(b.page);
    result.numbers.broken = { notice, ms: Date.now() - tErr, rev: still?.reload?.rev };
    check("a converter error is shown as a notice, the running skin stays", /did not build/.test(notice ?? "") && still?.active && (still?.reload?.rev ?? 0) === rev1,
      JSON.stringify(result.numbers.broken));
    writeFileSync(css, cssText);
    const fixed = await waitUi(b.page, (p) => (p.reload?.rev ?? 0) > rev1, 20000);
    result.numbers.fixed = { ms: fixed.ms, rev: fixed.probe?.reload?.rev, rect: await goldRect(b.page) };
    check("the fix brings the next build", fixed.ok, JSON.stringify(result.numbers.fixed));

    // 5. The watch server's path only.
    const others = requests.filter((p) => !/^\/pkg\/engraved\.(rev|ui2|errors\.json)$/.test(p));
    result.numbers.requests = { total: requests.length, others: others.slice(0, 5) };
    check("the page asks the watch server for the skin's package files only (no mods)", requests.length > 0 && others.length === 0, JSON.stringify(result.numbers.requests));
    await b.page.screenshot({ path: `${o.logDir}/ui2-reload.png` }).catch(() => {});
  } catch (e) {
    error = e;
  }
  if (b) {
    result.consoleErrors.push(...b.errors);
    const errs = b.errors.filter((e) => !e.includes("/pkg-mt/"));
    check("no console errors", errs.length === 0, `${errs.length} error(s)${errs[0] ? `: ${errs[0]}` : ""}`);
    await b.close();
  }
  stopWatch(watch);
  if (error) {
    check("no scenario error", false, error.message);
    result.error = error.stack ?? String(error);
  }
  result.pass = result.checks.every((x) => x.pass);
  result.durationMs = Date.now() - started;
  return result;
}
