// A/B bench against the original game (untracked helper): one fresh page on a running real-core server that holds an
// imported save, the normal client (original Lua/XML UI, WebGPU, High preset), headed Chrome, 1920x1080 at DPR 1.
//   node tools/playtest/ab-bench.mjs --url http://127.0.0.1:8080 --nick <nick> --token <token file> --out <dir>
//     --mode fps|turn [--uncapped] [--python python] [--turnwatch <turnwatch.py>] [--presentmon <PresentMon.exe>]
//     [--settle-s 10] [--phase-s 30] [--pin]
// fps : load, settle, idle 30 s (rAF + PresentMon), pan 30 s (ArrowRight 3 s / ArrowLeft 3 s x5, rAF + PresentMon).
// turn: load, settle, the screen watcher on the END TURN button, a real mouse click on it, until the new turn is playable.
// Writes <out>/result.json, console.log, screenshots, the watcher dir and the PresentMon CSVs.
import { spawn } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { openPage, state, appUrl } from "./lib/browser.mjs";
import { installProbe, now, drain, frames } from "./lib/bench.mjs";
import { uiDriver } from "./lib/ui-driver.mjs";

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const args = process.argv.slice(2);
const opt = (k, d) => {
  const i = args.indexOf(k);
  return i >= 0 ? args[i + 1] : d;
};
const flag = (k) => args.includes(k);
const o = {
  url: opt("--url", "http://127.0.0.1:8080"),
  nick: opt("--nick"),
  token: opt("--token"),
  out: path.resolve(opt("--out", "ab-out")),
  mode: opt("--mode", "fps"),
  uncapped: flag("--uncapped"),
  python: opt("--python", "python"),
  turnwatch: opt("--turnwatch"),
  presentmon: opt("--presentmon"),
  settleS: Number(opt("--settle-s", "10")),
  phaseS: Number(opt("--phase-s", "30")),
  pin: flag("--pin"),
};
if (!o.pin) process.env.CIV5_NO_CPU_PIN = "1";
fs.mkdirSync(o.out, { recursive: true });
const res = { mode: o.mode, uncapped: o.uncapped, pin: o.pin, startedAt: new Date().toISOString(), marks: {}, notes: [] };
const t = () => Date.now();
const save = () => fs.writeFileSync(path.join(o.out, "result.json"), JSON.stringify(res, null, 2));
const log = (s) => {
  const line = `[${new Date().toISOString()}] ${s}`;
  console.log(line);
  fs.appendFileSync(path.join(o.out, "run.log"), `${line}\n`);
};

async function presentMon(file, secs) {
  // An elevated helper (PresentMon sees no presents unelevated): a request file in its queue dir, then its .done file.
  const q = opt("--pm-queue");
  if (q && fs.existsSync(path.join(q, "ready"))) {
    const req = path.join(q, `${Date.now()}.req`);
    fs.writeFileSync(req, `${file}\n${secs}\n`);
    for (let i = 0; i < (secs + 30) * 20; i++) {
      if (fs.existsSync(`${req}.done`)) return { code: fs.readFileSync(`${req}.done`, "utf8").trim(), out: "elevated helper" };
      await sleep(50);
    }
    return { code: "timeout", out: "elevated helper" };
  }
  if (!o.presentmon) return null;
  return new Promise((resolve) => {
    const p = spawn(o.presentmon, ["--process_name", "chrome.exe", "--output_file", file, "--timed", String(secs), "--terminate_after_timed", "--no_console_stats"], { stdio: ["ignore", "pipe", "pipe"] });
    let err = "";
    p.stdout.on("data", (d) => (err += d));
    p.stderr.on("data", (d) => (err += d));
    p.on("exit", (code) => resolve({ code, out: err.trim() }));
    p.on("error", (e) => resolve({ code: -1, out: String(e) }));
  });
}

const b = await openPage({
  headed: true,
  uncapped: o.uncapped,
  logDir: o.out,
  viewport: { width: 1920, height: 1080 },
  chromeArgs: ["--force-device-scale-factor=1", "--window-position=0,0", "--window-size=1920,1200"],
});
const { page } = b;
res.chromeArgs = b.args;
res.chrome = b.browser.version();
let error = null;
try {
  const token = fs.readFileSync(o.token, "utf8").trim();
  await page.addInitScript(([nick, tok]) => {
    try {
      localStorage.setItem("civ5.nick", nick);
      localStorage.setItem(`civ5.token.${nick}`, tok);
    } catch {
      /* storage blocked */
    }
  }, [o.nick, token]);
  // The normal client: no `ui` (the original UI is the default), High preset; `test=1` for the hooks, `stats=1` for counters.
  const url = appUrl(o.url, { preset: "high" }, { nick: o.nick, stats: "1" });
  res.url = url;
  const t0 = t();
  res.marks.open = t0;
  await page.goto(url);
  let s = null;
  let last = "";
  const drv = uiDriver(b);
  // Load: until the game screen is up with the turn (playable), then until the original UI shows the END TURN button.
  while (t() - t0 < 300000) {
    s = await state(page);
    const line = s ? `${s.screen}/${s.net?.phase}/turn ${s.turn}/${s.status}` : "no hooks";
    if (line !== last) log(`state +${t() - t0} ms: ${line}`);
    last = line;
    if (s && s.screen === "play" && s.net?.phase === "play" && s.turn !== null) break;
    if (s?.fatal) throw new Error(`fatal: ${s.fatal}`);
    await sleep(100);
  }
  if (!s || s.screen !== "play") throw new Error(`no game: ${last}`);
  res.marks.play = t();
  res.loadPlayMs = res.marks.play - t0;
  res.turn0 = s.turn;
  let btn = null;
  while (t() - t0 < 300000) {
    btn = await drv.find("/ActionInfoPanel/EndTurnButton", (r) => /\/ActionInfoPanel\/EndTurnButton$/.test(r.path) && r.cls === "GridButton").catch(() => null);
    if (btn) break;
    await sleep(200);
  }
  res.marks.ui = t();
  res.loadUiMs = res.marks.ui - t0;
  // The terrain in view complete (`terrain.view_ready`) is when the map is drawn: wait for it (max 120 s).
  while (t() - t0 < 300000) {
    const v = await page.evaluate(() => window.__civ5stats?.counters?.["terrain.view_ready"] ?? null);
    if (v === 1) break;
    await sleep(100);
  }
  res.marks.viewReady = t();
  res.loadViewReadyMs = res.marks.viewReady - t0;
  log(`load: play ${res.loadPlayMs} ms, END TURN button ${res.loadUiMs} ms, terrain view ready ${res.loadViewReadyMs} ms`);
  res.viewport = await page.evaluate(() => ({ dpr: devicePixelRatio, iw: innerWidth, ih: innerHeight, ow: outerWidth, oh: outerHeight, sx: screenX, sy: screenY }));
  res.gpu = await page.evaluate(async () => {
    const a = await navigator.gpu?.requestAdapter();
    return a ? { vendor: a.info?.vendor, architecture: a.info?.architecture, description: a.info?.description } : null;
  });
  res.threads = await page.evaluate(() => ({ mode: window.__civ5threads?.mode ?? "single", coi: self.crossOriginIsolated === true }));
  await sleep(o.settleS * 1000);
  res.marks.settled = t();
  const st = await page.evaluate(() => {
    const s = window.__civ5stats ?? {};
    return { msaa: s.counters?.["render.msaa"] ?? null, preset: s.preset ?? s.settings?.preset ?? null, backend: s.backend ?? s.gfx ?? null, ui: s.ui ?? null, replica: s.replica ?? null, camera: s.camera ?? null };
  });
  res.stats0 = st;
  log(`settled: msaa ${st.msaa}, ui ${JSON.stringify(st.ui)}, threads ${JSON.stringify(res.threads)}, viewport ${JSON.stringify(res.viewport)}`);
  await page.screenshot({ path: path.join(o.out, "start.png") });

  if (o.mode === "fps") {
    await installProbe(page);
    await sleep(1000);
    const phase = async (name, fn) => {
      const pm = presentMon(path.join(o.out, `pm-${name}.csv`), o.phaseS);
      await sleep(300);
      const a = await now(page);
      const wall = t();
      await fn();
      const z = await now(page);
      res.marks[`${name}From`] = wall;
      const pmr = await pm;
      const data = await drain(page);
      const all = frames(data.ts, a, z);
      const skip5 = frames(data.ts, a + 5000, z);
      res[name] = { all, skip5, presentMon: pmr };
      log(`${name}: rAF p50 ${skip5.fpsP50} fps, 1% low ${skip5.fps1pLow} (skip 5 s; all: ${all.fpsP50}/${all.fps1pLow}, ${all.frames} frames); PresentMon ${pmr ? `exit ${pmr.code}` : "off"}`);
    };
    await phase("idle", () => sleep(o.phaseS * 1000));
    await page.screenshot({ path: path.join(o.out, "idle-end.png") });
    // The arrow keys go to the focused canvas (focus by script: a click on the map would be a game action).
    await page.evaluate(() => {
      const c = document.querySelector("canvas");
      if (c) {
        if (c.tabIndex < 0) c.tabIndex = 0;
        c.focus();
      }
    });
    await phase("pan", async () => {
      // `--pan-first left`: the left leg first (the start camera has sea and clouds to the east).
      const legs = opt("--pan-first", "right") === "left" ? ["ArrowLeft", "ArrowRight"] : ["ArrowRight", "ArrowLeft"];
      for (let i = 0; i < 5; i++) {
        for (const k of legs) {
          await page.keyboard.down(k);
          if (i === 0) {
            await sleep(1500);
            await page.screenshot({ path: path.join(o.out, `pan-${k}-mid.png`) });
            await sleep(1500);
          } else await sleep(3000);
          await page.keyboard.up(k);
        }
      }
    });
    await page.screenshot({ path: path.join(o.out, "pan-end.png") });
    const d = await drain(page);
    res.statsSamples = d.samples.filter((x, i) => i % 4 === 0).map((x) => ({ t: Math.round(x.t), draws: x.v["draws"], fps: x.v["fps"], cpu: x.v["frame.cpu_ms"], gpu: x.v["frame.gpu_ms"], focus: x.v["doc.focus"] }));
  } else {
    // Turn: the button's screen rectangle in physical pixels (DPR 1: CSS px = px; the content's origin on the screen).
    const scale = (await page.evaluate(() => window.__civ5stats?.camera?.ui_scale)) ?? 1;
    btn = await drv.find("/ActionInfoPanel/EndTurnButton", (r) => /\/ActionInfoPanel\/EndTurnButton$/.test(r.path) && r.cls === "GridButton");
    const texts = await drv.dump("/ActionInfoPanel/EndTurn");
    res.buttonBefore = { rect: btn, scale, texts: texts.filter((r) => r.text).map((r) => `${r.path} ${r.text}`) };
    const pad = 8;
    const css = { x0: Math.round(btn.x * scale) - pad, y0: Math.round(btn.y * scale) - pad, x1: Math.round((btn.x + btn.w) * scale) + pad, y1: Math.round((btn.y + btn.h) * scale) + pad };
    // Where the page sits on the screen: the page's own pixels of a patch around the button found in a screen grab.
    await page.screenshot({ path: path.join(o.out, "locate-page.png") });
    const big = { x0: Math.max(0, css.x0 - 200), y0: Math.max(0, css.y0 - 150), x1: Math.min(1920, css.x1 + 40), y1: Math.min(1080, css.y1 + 20) };
    const loc = await new Promise((resolve, reject) => {
      const p = spawn(o.python, [opt("--locate"), path.join(o.out, "locate-page.png"), String(big.x0), String(big.y0), String(big.x1), String(big.y1)], { stdio: ["ignore", "pipe", "inherit"] });
      let s = "";
      p.stdout.on("data", (d) => (s += d));
      p.on("exit", (c) => (c === 0 ? resolve(JSON.parse(s)) : reject(new Error(`locate exit ${c}`))));
    });
    res.locate = loc;
    log(`page origin on screen: ${JSON.stringify(loc)}`);
    const ox = loc.ox;
    const oy = loc.oy;
    const region = [ox + css.x0, oy + css.y0, ox + css.x1, oy + css.y1];
    res.watch = { css, origin: [ox, oy], region };
    await page.screenshot({ path: path.join(o.out, "button-page.png"), clip: { x: css.x0, y: css.y0, width: css.x1 - css.x0, height: css.y1 - css.y0 } });
    if (loc.err > 25) res.notes.push(`locate error ${loc.err}: check the watcher region by its frames`);
    const [bl] = await drv.lua(`local p = Players[Game.GetActivePlayer()] print(TAG .. tostring(p:GetEndTurnBlockingType()) .. "," .. tostring(Game.GetGameTurn()))`);
    res.blockingBefore = bl;
    log(`button ${JSON.stringify(btn)} scale ${scale}; region ${region.join(" ")}; blocking,turn ${bl}; ${res.buttonBefore.texts.join(" | ")}`);
    // Events of the turn change from the UI (printed when they fire; nothing polls the UI during the wait).
    await drv.lua(`if not __ab_listen then __ab_listen = true
      for _, name in ipairs({ "ActivePlayerTurnEnd", "ActivePlayerTurnStart", "NewGameTurn", "SerialEventEndTurnDirty" }) do
        Events[name].Add(function() print("ABEV " .. name .. " turn=" .. Game.GetGameTurn()) end)
      end end`);
    const wdir = path.join(o.out, "watch");
    const w = spawn(o.python, [o.turnwatch, "--region", ...region.map(String), "--out", wdir, "--seconds", "75"], { stdio: "inherit" });
    await sleep(3000);
    const consoleFrom = b.console.length;
    const cx = Math.round((btn.x + btn.w / 2) * scale);
    const cy = Math.round((btn.y + btn.h / 2) * scale);
    await page.mouse.move(cx, cy);
    await sleep(300);
    res.marks.clickDown = t();
    await page.mouse.down();
    await sleep(60);
    await page.mouse.up();
    res.marks.click = t();
    log(`clicked END TURN at (${cx},${cy})`);
    // The client's state: the turn number and its status, sampled every 100 ms.
    const samples = [];
    let lastKey = "";
    let turnAt = null;
    const turn0 = res.turn0;
    while (t() - res.marks.click < 120000) {
      const x = await state(page);
      const key = x ? `turn ${x.turn} status ${x.status} endTurn ${JSON.stringify(x.endTurn)} ts ${JSON.stringify(x.net?.turnState ? { turn: x.net.turnState.turn, phase: x.net.turnState.phase } : null)} orders ${x.net?.ordersEnabled}` : "no state";
      if (key !== lastKey) {
        samples.push({ ms: t() - res.marks.click, key });
        log(`  +${t() - res.marks.click} ms ${key}`);
      }
      lastKey = key;
      if (x && x.turn > turn0 && turnAt === null) turnAt = t();
      if (turnAt !== null && t() - turnAt > 8000) break;
      await sleep(100);
    }
    res.stateTimeline = samples;
    res.marks.turnChanged = turnAt;
    // After the turn: the button text now (a Lua read once the turn has settled).
    const after = await drv.dump("/ActionInfoPanel/EndTurn").catch(() => []);
    res.buttonAfter = after.filter((r) => r.text).map((r) => `${r.path} ${r.text}`);
    res.events = b.console.slice(consoleFrom).filter((l) => /ABEV|turn|Turn/.test(l.text)).map((l) => ({ ms: b.t0 + l.at - res.marks.click, text: l.text.slice(0, 200) }));
    await page.screenshot({ path: path.join(o.out, "after.png") });
    await new Promise((r) => w.on("exit", r));
    res.watchStart = Number(fs.readFileSync(path.join(wdir, "start.txt"), "utf8")) * 1000;
    res.clickInWatchS = (res.marks.click - res.watchStart) / 1000;
    log(`watcher started ${res.watchStart}; click at ${res.clickInWatchS.toFixed(3)} s of the watcher log`);
  }
  res.fatal = (await state(page))?.fatal ?? null;
  res.statsEnd = await page.evaluate(() => {
    const s = window.__civ5stats ?? {};
    return { ui: s.ui ?? null, replica: s.replica ?? null };
  });
} catch (e) {
  error = e;
  res.error = e.stack ?? String(e);
  log(`ERROR ${res.error}`);
}
res.consoleErrors = b.errors.slice(0, 50);
res.finishedAt = new Date().toISOString();
save();
await b.close();
process.exit(error ? 1 : 0);
