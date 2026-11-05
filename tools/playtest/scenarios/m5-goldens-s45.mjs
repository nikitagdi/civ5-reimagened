// `m5-goldens-s45` (M5 WO-I, AC-S2): scenes S4 (tech tree) and S5 (trade with the AI) of the original's goldens
// (`assets-out/goldens-b4-ui/`): the scene's `.Civ5Save` is a sleeping game of the server (`tools/playtest/m5/import-golden.ps1`: the
// game of nick g4 / g5), the page logs in, takes the seat (the game wakes from the save), opens the window the way a player does,
// shoots 1920x1080 and compares with the golden by FLIP (`lib/flip.mjs`). The rows live in `lib/m5-ui-scenes.mjs`, so that
// WO-H's `m5-goldens` (S1-S3) can run them next to its own.
//   node tools/playtest/run.mjs --attach http://127.0.0.1:8862 --scenario m5-goldens-s45 [--golden-captures S4,S5] [--gfx webgl2]
import { runUiScenes } from "../lib/m5-ui-scenes.mjs";

export async function run(ctx) {
  const o = ctx.opts;
  const result = { scenario: "m5-goldens-s45", pass: false, logs: o.logDir, checks: [], numbers: {}, consoleErrors: [] };
  const base = o.attach ?? ctx.server?.url;
  if (!base) {
    result.checks.push({ name: "options", pass: false, detail: "m5-goldens-s45 needs --attach <url of the server with the imported saves of S4 and S5>" });
    return result;
  }
  const only = o.goldenCaptures ? o.goldenCaptures.split(",").map((s) => s.trim()) : null;
  return runUiScenes({ ...ctx, result, base, only });
}
