// `m5-goldens` (milestone M5, WO-H S1-S3, WO-I S4-S5; BUILD B4 AC-S2): our client against the frames of the original game.
// For each scene of `lib/goldens.mjs` (`SCENES`): the save of the original's multiplayer game is woken on the server
// (`m5/make-golden-state.ps1`, `m5/golden-server.ps1`), the scene is set up by the scene's own `shoot` (camera, selection, windows),
// a 1920x1080 frame is taken and compared with the golden by FLIP (`m5/flip.py`, NVIDIA flip-evaluator).
//   node tools/playtest/run.mjs --attach http://127.0.0.1:8850 --scenario m5-goldens [--golden-scenes S1,S2] [--swiftshader]
// Output per scene in `target/m5-goldens/<id>/`: golden.png, ours.png, flip.png (the error map), side.png (golden | ours | map),
// flip.json (the numbers). Checks: ui.lua_errors == 0, replica.diverged == 0, the FLIP numbers against `m5/thresholds.json`
// (the threshold of a scene is set from the first scenes, see that file's notes; `--golden-no-threshold` only records).
import { copyFileSync, existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { execFileSync } from "node:child_process";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { openPage } from "../lib/browser.mjs";
import { GAMES, GOLDEN_DIR, SCENES, openGame } from "../lib/goldens.mjs";

const here = path.dirname(fileURLToPath(import.meta.url));
const repo = path.resolve(here, "../../..");
const THRESHOLDS = path.join(here, "../m5/thresholds.json");

export function flip(goldenPng, oursPng, outDir, hide = []) {
  const args = [path.join(here, "../m5/flip.py"), goldenPng, oursPng, outDir];
  for (const r of hide) args.push("--hide", r.join(","));
  const py = process.env.CIV5_PYTHON ?? "python";
  const pylib = process.env.CIV5_FLIP_LIB ?? path.join(repo, "target/m5/pylib");
  const env = { ...process.env, PYTHONPATH: [pylib, process.env.PYTHONPATH].filter(Boolean).join(path.delimiter) };
  const out = execFileSync(py, args, { env, encoding: "utf8", maxBuffer: 1 << 24 }).trim().split("\n").pop();
  return JSON.parse(out);
}

export async function run(ctx) {
  const o = ctx.opts;
  const started = Date.now();
  const result = { scenario: "m5-goldens", pass: false, logs: o.logDir, checks: [], numbers: { scenes: {} }, consoleErrors: [] };
  const check = (name, pass, detail) => result.checks.push({ name, pass: !!pass, detail: detail ?? "" });
  const base = o.attach ?? ctx.server?.url;
  if (!base) { check("options", false, "m5-goldens needs --attach <url of the server with the golden games>"); return result; }
  const want = o.goldenScenes ? new Set(o.goldenScenes.split(",").map((s) => s.trim())) : null;
  const scenes = SCENES.filter((s) => !want || want.has(s.id));
  const thresholds = existsSync(THRESHOLDS) && !o.goldenNoThreshold ? JSON.parse(readFileSync(THRESHOLDS, "utf8")) : null;
  const outRoot = path.resolve(o.goldenOut ?? path.join(repo, "target/m5-goldens"));
  // the scenes of one game share one session, in list order
  const keys = [...new Set(scenes.map((s) => s.game))];
  for (const key of keys) {
    const mine = scenes.filter((s) => s.game === key);
    const b = await openPage({ headed: o.headed, swiftshader: o.swiftshader, logDir: o.logDir, consoleFile: `m5-goldens-${key}.console.log`, viewport: { width: 1920, height: 1080 } });
    let error = null;
    try {
      let g = null;
      for (let attempt = 0; ; attempt++) {
        try { g = await openGame(b, base, key, o); break; } catch (e) {
          // the seat's nick is held for a while after a session of it ended (NickTaken): wait and sign in again
          if (attempt >= 4) throw e;
          await new Promise((r) => setTimeout(r, 30000));
        }
      }
      await g.advanceTo(GAMES[key].goal);
      for (const sc of mine) {
        const dir = path.join(outRoot, sc.id);
        mkdirSync(dir, { recursive: true });
        const golden = path.join(GOLDEN_DIR, sc.golden);
        try {
          await sc.shoot(g);
          const ours = path.join(dir, "ours.png");
          await b.page.screenshot({ path: ours });
          copyFileSync(golden, path.join(dir, "golden.png"));
          const f = flip(golden, ours, dir, sc.hide ?? []);
          writeFileSync(path.join(dir, "flip.json"), JSON.stringify({ scene: sc.id, title: sc.title, notes: sc.notes ?? "", ...f }, null, 1));
          result.numbers.scenes[sc.id] = f;
          const st = await g.stats();
          check(`${sc.id}: ui.lua_errors == 0`, st.ui?.lua_errors === 0, `${st.ui?.lua_errors}`);
          check(`${sc.id}: replica.diverged == 0`, st.replica?.diverged === 0, `${st.replica?.diverged}`);
          const t = thresholds?.scenes?.[sc.id] ?? thresholds?.default;
          if (t) {
            for (const [k, lim] of Object.entries(t)) {
              const v = k.includes(".") ? k.split(".").reduce((a, p) => a?.[p], f) : f[k];
              check(`${sc.id}: FLIP ${k} ${v} <= ${lim}`, typeof v === "number" && v <= lim, `${v}`);
            }
          } else check(`${sc.id}: FLIP recorded (no threshold)`, true, `mean ${f.mean}`);
        } catch (e) {
          check(`${sc.id}: scene`, false, e.message.split("\n")[0]);
          try { await b.page.screenshot({ path: path.join(dir, "failure.png") }); } catch { /* gone */ }
        }
      }
    } catch (e) {
      error = e;
      check(`game ${key}`, false, e.message.split("\n")[0]);
    }
    result.consoleErrors.push(...b.errors);
    await b.close();
    if (error) result.error = error.stack ?? String(error);
  }
  check("no console errors", result.consoleErrors.length === 0, `${result.consoleErrors.length} error(s)${result.consoleErrors[0] ? `: ${result.consoleErrors[0]}` : ""}`);
  result.pass = result.checks.every((c) => c.pass);
  result.durationMs = Date.now() - started;
  return result;
}
